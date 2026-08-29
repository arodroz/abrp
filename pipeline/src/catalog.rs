//! Catalog writer: `catalog.json`, the manifest tying one region's
//! artifacts (Region Pack, Charger Pack, Map Pack) to the shared OSM
//! snapshot epoch and Protomaps build (ADR 0005/0007/0008). Written last by
//! `bin/build_packs.rs`, after whichever jobs ran this session; merges into
//! an existing `catalog.json` so a partial run (e.g. `--jobs chargers`)
//! only replaces the artifact entries it actually produced, and inherits
//! `osm_snapshot_epoch`/`protomaps_build` from a prior run when this run
//! didn't refresh them.
//!
//! Fails closed (wayfinder H-05): an existing catalog that can't be read or
//! parsed is a hard error, never silently treated as "no catalog" -- a
//! corrupt catalog must never be replaced by a partial one written on top
//! of it. A run that doesn't produce every required artifact (a "partial"
//! job set, e.g. `--jobs chargers`) is only accepted when a valid existing
//! catalog for the same region already supplies what this run didn't
//! produce; otherwise the run is rejected with a message telling the
//! operator to run a full build. The final catalog -- whether from a full
//! or a completed partial build -- is validated (nonzero epoch, a nonempty
//! Protomaps build, every required artifact present with a well-formed
//! sha256) before being written via a temp file plus atomic rename.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CATALOG_FORMAT: u32 = 1;
pub const REGION_PACK_FORMAT: &str = "rpack-1.1";
pub const CHARGER_PACK_FORMAT: &str = "cpack-1";
pub const MAP_PACK_FORMAT: &str = "pmtiles-z14";

/// The artifacts every installable catalog must carry.
pub const REQUIRED_ARTIFACTS: &[&str] = &["region_pack", "charger_pack", "map_pack"];

#[derive(Debug)]
pub enum CatalogError {
    Io(io::Error),
    /// An existing catalog file could not be parsed -- a hard error, since
    /// it must never be silently treated as "no catalog" and overwritten.
    Corrupt {
        path: String,
        reason: String,
    },
    /// This run's job set didn't produce every required artifact, and
    /// there was no valid existing catalog to complete it from.
    PartialWithoutValidBase(String),
    /// This run's job set is partial and an existing catalog exists, but
    /// it's for a different region.
    WrongRegion {
        existing: String,
        requested: String,
    },
    /// The catalog that would be written fails a required invariant.
    Invalid(String),
    Serialize(serde_json::Error),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::Io(e) => write!(f, "io error: {e}"),
            CatalogError::Corrupt { path, reason } => {
                write!(f, "existing catalog at {path} is corrupt: {reason}")
            }
            CatalogError::PartialWithoutValidBase(msg) => write!(f, "{msg}"),
            CatalogError::WrongRegion {
                existing,
                requested,
            } => write!(
                f,
                "existing catalog is for region {existing:?}, but this run is for region {requested:?}"
            ),
            CatalogError::Invalid(msg) => write!(f, "refusing to write an invalid catalog: {msg}"),
            CatalogError::Serialize(e) => write!(f, "failed to serialize catalog: {e}"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<io::Error> for CatalogError {
    fn from(e: io::Error) -> Self {
        CatalogError::Io(e)
    }
}

impl From<serde_json::Error> for CatalogError {
    fn from(e: serde_json::Error) -> Self {
        CatalogError::Serialize(e)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactEntry {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Formats {
    pub region_pack: String,
    pub charger_pack: String,
    pub map_pack: String,
}

impl Default for Formats {
    fn default() -> Self {
        Formats {
            region_pack: REGION_PACK_FORMAT.to_string(),
            charger_pack: CHARGER_PACK_FORMAT.to_string(),
            map_pack: MAP_PACK_FORMAT.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub catalog_format: u32,
    pub region_id: String,
    pub region_name: String,
    pub osm_snapshot_epoch: u64,
    pub protomaps_build: String,
    pub built_at_epoch: u64,
    pub artifacts: BTreeMap<String, ArtifactEntry>,
    pub formats: Formats,
}

/// Streamed sha256 -- never loads the whole file into memory (the corridor
/// rpack is 500+ MB).
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn artifact_entry(path: &Path) -> io::Result<ArtifactEntry> {
    let file_name = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default()
        .to_string();
    Ok(ArtifactEntry {
        file: file_name,
        bytes: path.metadata()?.len(),
        sha256: sha256_file(path)?,
    })
}

/// Reads `catalog_path` if it exists and parses as a `Catalog`. `Ok(None)`
/// means there is no existing catalog (`NotFound`) -- fine to proceed with
/// a full build. Any other I/O failure, or a file that exists but fails to
/// parse, is a hard `Err`: a corrupt catalog must never be silently
/// treated as "no existing catalog" and overwritten.
fn read_existing(catalog_path: &Path) -> Result<Option<Catalog>, CatalogError> {
    let bytes = match std::fs::read(catalog_path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CatalogError::Io(e)),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| CatalogError::Corrupt {
            path: catalog_path.display().to_string(),
            reason: e.to_string(),
        })
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Validates the catalog that's about to be written: `osm_snapshot_epoch`
/// is nonzero, `protomaps_build` is nonempty, and every required artifact
/// (region_pack, charger_pack, map_pack) is present with a nonempty file
/// name and a well-formed sha256 -- mirroring what the installer needs to
/// accept the catalog. A catalog failing this is not installable and must
/// never be written, whether it came from a full build or a partial one
/// completed against an existing base.
fn validate_catalog(catalog: &Catalog) -> Result<(), CatalogError> {
    if catalog.osm_snapshot_epoch == 0 {
        return Err(CatalogError::Invalid(
            "osm_snapshot_epoch is zero".to_string(),
        ));
    }
    if catalog.protomaps_build.is_empty() {
        return Err(CatalogError::Invalid(
            "protomaps_build is empty".to_string(),
        ));
    }
    for name in REQUIRED_ARTIFACTS {
        let entry = catalog
            .artifacts
            .get(*name)
            .ok_or_else(|| CatalogError::Invalid(format!("missing required artifact {name}")))?;
        if entry.file.is_empty() {
            return Err(CatalogError::Invalid(format!(
                "artifact {name} has an empty file name"
            )));
        }
        if !is_sha256_hex(&entry.sha256) {
            return Err(CatalogError::Invalid(format!(
                "artifact {name} has a malformed sha256: {:?}",
                entry.sha256
            )));
        }
    }
    Ok(())
}

/// Writes `bytes` to `path` via a temp file in the same directory plus an
/// atomic rename, so a reader never observes a partially written catalog.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut f = File::create(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)
}

/// Writes `catalog_path`. `new_artifacts` is (logical artifact name, file
/// path) for the artifacts this run produced.
///
/// If `new_artifacts` covers every entry in `REQUIRED_ARTIFACTS`, this is a
/// full build: the catalog is written from scratch (an existing catalog's
/// contents, if any, are ignored once its own readability has been
/// confirmed). Otherwise this is a partial build (e.g. `--jobs chargers`),
/// which is only accepted when a valid existing catalog for the same
/// `region_id` exists to supply the artifacts/fields this run didn't
/// produce; its untouched artifact entries are preserved byte-for-byte
/// (H-05). `osm_snapshot_epoch`/`protomaps_build` of `None` inherit that
/// base catalog's value. The final catalog is validated and written via a
/// temp file plus atomic rename.
pub fn write_catalog(
    catalog_path: &Path,
    region_id: &str,
    region_name: &str,
    osm_snapshot_epoch: Option<u64>,
    protomaps_build: Option<&str>,
    built_at_epoch: u64,
    new_artifacts: &[(&str, &Path)],
) -> Result<(), CatalogError> {
    let existing = read_existing(catalog_path)?;

    let is_full_build = REQUIRED_ARTIFACTS
        .iter()
        .all(|req| new_artifacts.iter().any(|(name, _)| name == req));

    let base = if is_full_build {
        None
    } else {
        match &existing {
            Some(c) if c.region_id == region_id => Some(c),
            Some(c) => {
                return Err(CatalogError::WrongRegion {
                    existing: c.region_id.clone(),
                    requested: region_id.to_string(),
                })
            }
            None => {
                return Err(CatalogError::PartialWithoutValidBase(format!(
                    "no existing catalog at {} to complete this partial build ({}); run a full build first",
                    catalog_path.display(),
                    new_artifacts
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            }
        }
    };

    let mut artifacts = base.map(|c| c.artifacts.clone()).unwrap_or_default();
    for (name, path) in new_artifacts {
        artifacts.insert((*name).to_string(), artifact_entry(path)?);
    }

    let osm_snapshot_epoch = osm_snapshot_epoch
        .or_else(|| base.map(|c| c.osm_snapshot_epoch))
        .unwrap_or(0);
    let protomaps_build = protomaps_build
        .map(str::to_string)
        .or_else(|| base.map(|c| c.protomaps_build.clone()))
        .unwrap_or_default();

    let catalog = Catalog {
        catalog_format: CATALOG_FORMAT,
        region_id: region_id.to_string(),
        region_name: region_name.to_string(),
        osm_snapshot_epoch,
        protomaps_build,
        built_at_epoch,
        artifacts,
        formats: Formats::default(),
    };

    validate_catalog(&catalog)?;

    write_atomic(catalog_path, &serde_json::to_vec_pretty(&catalog)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_artifact(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// A full lu-dev job set: all three required artifacts.
    fn full_lu_dev_artifacts(dir: &Path) -> [(&'static str, std::path::PathBuf); 3] {
        [
            (
                "region_pack",
                write_artifact(dir, "lu-dev.rpack", b"hello world"),
            ),
            (
                "charger_pack",
                write_artifact(dir, "lu-dev-chargers.json", b"second file"),
            ),
            (
                "map_pack",
                write_artifact(dir, "lu-dev.pmtiles", b"third file"),
            ),
        ]
    }

    #[test]
    fn sha256_file_matches_a_known_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, b"hello world").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn absent_catalog_and_a_full_build_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = full_lu_dev_artifacts(dir.path());
        let refs: Vec<(&str, &Path)> = artifacts.iter().map(|(n, p)| (*n, p.as_path())).collect();
        let catalog_path = dir.path().join("catalog.json");

        write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            Some(1_700_000_000),
            Some("20260827"),
            1_700_000_100,
            &refs,
        )
        .unwrap();

        let catalog: Catalog =
            serde_json::from_slice(&std::fs::read(&catalog_path).unwrap()).unwrap();
        assert_eq!(catalog.catalog_format, 1);
        assert_eq!(catalog.region_id, "lu-dev");
        assert_eq!(catalog.osm_snapshot_epoch, 1_700_000_000);
        assert_eq!(catalog.protomaps_build, "20260827");
        assert_eq!(
            catalog.artifacts["region_pack"].sha256,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(catalog.artifacts["region_pack"].bytes, 11);
        assert_eq!(catalog.formats.region_pack, "rpack-1.1");
    }

    #[test]
    fn absent_catalog_and_a_partial_build_fails() {
        let dir = tempfile::tempdir().unwrap();
        let chargers = write_artifact(dir.path(), "lu-dev-chargers.json", b"chargers");
        let catalog_path = dir.path().join("catalog.json");

        let err = write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            None,
            None,
            1,
            &[("charger_pack", &chargers)],
        )
        .unwrap_err();

        assert!(matches!(err, CatalogError::PartialWithoutValidBase(_)));
        assert!(!catalog_path.exists());
    }

    #[test]
    fn corrupt_existing_catalog_fails_even_a_full_build() {
        let dir = tempfile::tempdir().unwrap();
        let catalog_path = dir.path().join("catalog.json");
        std::fs::write(&catalog_path, b"{ not valid json").unwrap();
        let artifacts = full_lu_dev_artifacts(dir.path());
        let refs: Vec<(&str, &Path)> = artifacts.iter().map(|(n, p)| (*n, p.as_path())).collect();

        let err = write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            Some(1),
            Some("20260827"),
            1,
            &refs,
        )
        .unwrap_err();

        assert!(matches!(err, CatalogError::Corrupt { .. }));
        // The corrupt file was never touched.
        assert_eq!(
            std::fs::read(&catalog_path).unwrap(),
            b"{ not valid json".to_vec()
        );
    }

    #[test]
    fn wrong_region_existing_catalog_fails_a_partial_build() {
        let dir = tempfile::tempdir().unwrap();
        let catalog_path = dir.path().join("catalog.json");
        let region = write_artifact(dir.path(), "corridor.rpack", b"hello world");
        let chargers = write_artifact(dir.path(), "corridor-chargers.json", b"second file");
        let map = write_artifact(dir.path(), "corridor.pmtiles", b"third file");
        write_catalog(
            &catalog_path,
            "corridor",
            "LU+BE+NL corridor",
            Some(1),
            Some("20260827"),
            1,
            &[
                ("region_pack", &region),
                ("charger_pack", &chargers),
                ("map_pack", &map),
            ],
        )
        .unwrap();

        // A partial lu-dev build landing on the corridor catalog.
        let lu_chargers = write_artifact(dir.path(), "lu-dev-chargers.json", b"lu chargers");
        let err = write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            None,
            None,
            2,
            &[("charger_pack", &lu_chargers)],
        )
        .unwrap_err();

        assert!(matches!(err, CatalogError::WrongRegion { .. }));
    }

    #[test]
    fn valid_partial_replacement_preserves_untouched_artifacts_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let catalog_path = dir.path().join("catalog.json");
        let artifacts = full_lu_dev_artifacts(dir.path());
        let refs: Vec<(&str, &Path)> = artifacts.iter().map(|(n, p)| (*n, p.as_path())).collect();
        write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            Some(42),
            Some("20260827"),
            1,
            &refs,
        )
        .unwrap();
        let before: Catalog =
            serde_json::from_slice(&std::fs::read(&catalog_path).unwrap()).unwrap();

        // A second, chargers-only run: no fresh epoch/build, only a new artifact.
        let new_chargers =
            write_artifact(dir.path(), "lu-dev-chargers.json", b"second run chargers");
        write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            None,
            None,
            2,
            &[("charger_pack", &new_chargers)],
        )
        .unwrap();

        let after: Catalog =
            serde_json::from_slice(&std::fs::read(&catalog_path).unwrap()).unwrap();
        // Inherited from the first run.
        assert_eq!(after.osm_snapshot_epoch, 42);
        assert_eq!(after.protomaps_build, "20260827");
        assert_eq!(after.built_at_epoch, 2);
        // The untouched artifacts survive the merge byte-for-byte.
        assert_eq!(
            after.artifacts["region_pack"],
            before.artifacts["region_pack"]
        );
        assert_eq!(after.artifacts["map_pack"], before.artifacts["map_pack"]);
        // The touched artifact was replaced with the new file's hash.
        assert_ne!(
            after.artifacts["charger_pack"],
            before.artifacts["charger_pack"]
        );
    }
}
