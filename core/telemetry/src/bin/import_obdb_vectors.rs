//! Mechanical OBDb test-vector importer (wayfinder #77, ADR 0013 point 4,
//! licensing lane 2). Reads recorded test cases out of a local clone of an
//! OBDb vehicle repo (`tests/test_cases/<year>/commands/*.yaml`) and
//! writes them as self-contained per-command JSON fixtures under
//! `data/imported/obdb/<vehicle>/`, recording the source repo URL and
//! commit SHA. Imports test *vectors* only -- never the `signalsets/v3`
//! JSON itself (bulk signalset import is explicitly deferred, ADR 0013
//! point 4). Output is tool-generated: never hand-edit it, regenerate it
//! instead (see `data/imported/obdb/NOTICE.md` for the exact command).
//!
//! Usage:
//! ```text
//! cargo run -p telemetry --bin import_obdb_vectors --features import-tools -- \
//!   --repo /path/to/cloned/Hyundai-IONIQ-5 \
//!   --source-url https://github.com/OBDb/Hyundai-IONIQ-5 \
//!   --year 2023 \
//!   --out data/imported/obdb/hyundai-ioniq5 \
//!   --count 5 \
//!   7E4.7EC.220101 7E4.7EC.220102 ...
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct ObdbTestFile {
    command_id: String,
    test_cases: Vec<ObdbTestCase>,
}

#[derive(Debug, Deserialize)]
struct ObdbTestCase {
    expected_values: BTreeMap<String, f64>,
    response: String,
}

#[derive(Debug, Serialize)]
struct ImportedVectorFile {
    source_repo: String,
    source_commit: String,
    command_id: String,
    tx_header: String,
    rx_header: String,
    request_hex: String,
    cases: Vec<ImportedCase>,
}

#[derive(Debug, Serialize)]
struct ImportedCase {
    response_lines: Vec<String>,
    expected_values: BTreeMap<String, f64>,
}

struct Args {
    repo: PathBuf,
    source_url: String,
    year: String,
    out: PathBuf,
    count: usize,
    commands: Vec<String>,
}

fn parse_args() -> Args {
    let mut repo = None;
    let mut source_url = None;
    let mut year = "2023".to_string();
    let mut out = None;
    let mut count = 5usize;
    let mut commands = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => repo = args.next().map(PathBuf::from),
            "--source-url" => source_url = args.next(),
            "--year" => year = args.next().expect("--year needs a value"),
            "--out" => out = args.next().map(PathBuf::from),
            "--count" => {
                count = args
                    .next()
                    .expect("--count needs a value")
                    .parse()
                    .expect("--count must be a number")
            }
            other => commands.push(other.to_string()),
        }
    }

    Args {
        repo: repo.expect("--repo <path to cloned OBDb vehicle repo> is required"),
        source_url: source_url.expect("--source-url <repo URL> is required"),
        year,
        out: out.expect("--out <output directory> is required"),
        count,
        commands: {
            assert!(
                !commands.is_empty(),
                "list at least one command id, e.g. 7E4.7EC.220101"
            );
            commands
        },
    }
}

fn commit_sha(repo: &Path) -> String {
    let output = Command::new("git")
        .args([
            "-C",
            repo.to_str().expect("utf8 repo path"),
            "rev-parse",
            "HEAD",
        ])
        .output()
        .expect("git rev-parse failed to run");
    assert!(
        output.status.success(),
        "git rev-parse HEAD failed in {repo:?}"
    );
    String::from_utf8(output.stdout)
        .expect("git output is utf8")
        .trim()
        .to_string()
}

/// Finds `<repo>/tests/test_cases/<year>/commands/<prefix>[|...].yaml` --
/// OBDb suffixes the bare `hdr.rax.cmd` id with flags like `|fc=1` or
/// `|f=-2024` when they apply, so this matches on the part before `|`.
fn find_test_file(repo: &Path, year: &str, prefix: &str) -> PathBuf {
    let dir = repo.join("tests/test_cases").join(year).join("commands");
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("reading {dir:?}: {e}"));
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let base = stem.split('|').next().unwrap_or(stem);
        if base == prefix {
            return path;
        }
    }
    panic!("no test case file for {prefix:?} under {dir:?}");
}

/// A minimal, tolerant re-implementation of `elm::parse_frame`'s
/// well-formedness check (3 hex-digit id + 2-16 even hex data digits),
/// used only to drop OBDb's occasional cosmetic leading "bare command"
/// line (e.g. `220101`) from the recorded response -- everything else is
/// kept verbatim. Getting this filter wrong is harmless either way: the
/// real engine (`crate::elm::classify_line`) already ignores any line
/// that isn't a well-formed frame for the target it's listening to.
fn looks_like_frame(line: &str) -> bool {
    let stripped: String = line.chars().filter(|&c| c != ' ').collect();
    if stripped.len() < 5 || !stripped.is_char_boundary(3) {
        return false;
    }
    let (id, data) = stripped.split_at(3);
    id.chars().all(|c| c.is_ascii_hexdigit())
        && (2..=16).contains(&data.len())
        && data.len().is_multiple_of(2)
        && data.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_hex_bytes(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd-length hex string {s:?}");
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).unwrap_or_else(|e| panic!("bad hex in {s:?}: {e}"))
        })
        .collect()
}

/// Splits `<hdr>.<rax>.<cmd_hex>[|...]` into headers and UDS request bytes
/// (service byte + hex-pair parameters, e.g. `"220101"` -> `22 01 01`).
/// Only the two-hex-digit-service-plus-DID shape this ticket's commands
/// use is handled -- sufficient for a mechanical, single-vehicle importer.
fn parse_command_id(id: &str) -> (String, String, String) {
    let base = id.split('|').next().unwrap_or(id);
    let mut parts = base.split('.');
    let hdr = parts.next().expect("command id has a header");
    let rax = parts.next().expect("command id has a receive address");
    let cmd_hex = parts.next().expect("command id has a service+DID");
    (hdr.to_string(), rax.to_string(), cmd_hex.to_string())
}

fn main() {
    let args = parse_args();
    std::fs::create_dir_all(&args.out).expect("creating output directory");
    let commit = commit_sha(&args.repo);

    for prefix in &args.commands {
        let path = find_test_file(&args.repo, &args.year, prefix);
        let yaml =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parsed: ObdbTestFile =
            serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parsing {path:?}: {e}"));

        let (hdr, rax, cmd_hex) = parse_command_id(&parsed.command_id);
        // A sanity check, not a hard requirement: confirms the UDS bytes we
        // derive from the id parse cleanly (catches a malformed command id
        // loudly instead of writing a bad fixture).
        let _ = parse_hex_bytes(&cmd_hex);

        let cases: Vec<ImportedCase> = parsed
            .test_cases
            .into_iter()
            .take(args.count)
            .map(|case| ImportedCase {
                response_lines: case
                    .response
                    .lines()
                    .filter(|l| looks_like_frame(l))
                    .map(str::to_string)
                    .collect(),
                expected_values: case.expected_values,
            })
            .collect();

        let out_file = ImportedVectorFile {
            source_repo: args.source_url.clone(),
            source_commit: commit.clone(),
            command_id: parsed.command_id.clone(),
            tx_header: hdr,
            rx_header: rax,
            request_hex: cmd_hex,
            cases,
        };

        let out_path = args.out.join(format!("{prefix}.json"));
        let json =
            serde_json::to_string_pretty(&out_file).expect("serializing imported vector file");
        std::fs::write(&out_path, json + "\n")
            .unwrap_or_else(|e| panic!("writing {out_path:?}: {e}"));
        println!("wrote {out_path:?} ({} cases)", out_file.cases.len());
    }
}
