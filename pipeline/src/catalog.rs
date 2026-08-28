//! Catalog writer: `catalog.json`, the manifest tying one region's
//! artifacts (Region Pack, Charger Pack, Map Pack) to the shared OSM
//! snapshot epoch and Protomaps build (ADR 0005/0007/0008). Written last by
//! `bin/build_packs.rs`, after whichever jobs ran this session; merges into
//! an existing `catalog.json` so a partial run (e.g. `--jobs chargers`)
//! only replaces the artifact entries it actually produced, and inherits
//! `osm_snapshot_epoch`/`protomaps_build` from a prior run when this run
//! didn't refresh them.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CATALOG_FORMAT: u32 = 1;
pub const REGION_PACK_FORMAT: &str = "rpack-1.1";
pub const CHARGER_PACK_FORMAT: &str = "cpack-1";
pub const MAP_PACK_FORMAT: &str = "pmtiles-z14";

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Reads `catalog_path` if it exists and parses as a `Catalog`; `None` on
/// any I/O or parse error (treated as "no existing catalog", so a missing
/// or corrupt file is simply overwritten rather than blocking the run).
fn read_existing(catalog_path: &Path) -> Option<Catalog> {
    let bytes = std::fs::read(catalog_path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes `catalog_path`. `new_artifacts` is (logical artifact name, file
/// path) for the artifacts this run produced -- their entries replace any
/// same-named entry in an existing catalog; entries from a prior run that
/// this run didn't touch are preserved. `osm_snapshot_epoch`/
/// `protomaps_build` of `None` inherit the existing catalog's value (or
/// `0`/empty if there is none), since a partial run (e.g. `--jobs
/// chargers`) doesn't refresh either.
pub fn write_catalog(
    catalog_path: &Path,
    region_id: &str,
    region_name: &str,
    osm_snapshot_epoch: Option<u64>,
    protomaps_build: Option<&str>,
    built_at_epoch: u64,
    new_artifacts: &[(&str, &Path)],
) -> io::Result<()> {
    let existing = read_existing(catalog_path);

    let mut artifacts = existing
        .as_ref()
        .map(|c| c.artifacts.clone())
        .unwrap_or_default();
    for (name, path) in new_artifacts {
        artifacts.insert((*name).to_string(), artifact_entry(path)?);
    }

    let osm_snapshot_epoch = osm_snapshot_epoch
        .or_else(|| existing.as_ref().map(|c| c.osm_snapshot_epoch))
        .unwrap_or(0);
    let protomaps_build = protomaps_build
        .map(str::to_string)
        .or_else(|| existing.as_ref().map(|c| c.protomaps_build.clone()))
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

    std::fs::write(catalog_path, serde_json::to_vec_pretty(&catalog)?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn write_catalog_records_both_artifacts_with_correct_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let region = dir.path().join("lu-dev.rpack");
        let chargers = dir.path().join("lu-dev-chargers.json");
        std::fs::write(&region, b"hello world").unwrap();
        std::fs::write(&chargers, b"second file").unwrap();
        let catalog_path = dir.path().join("catalog.json");

        write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            Some(1_700_000_000),
            Some("20260827"),
            1_700_000_100,
            &[("region_pack", &region), ("charger_pack", &chargers)],
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
    fn write_catalog_merges_into_an_existing_file_without_epoch_or_build() {
        let dir = tempfile::tempdir().unwrap();
        let catalog_path = dir.path().join("catalog.json");
        let region = dir.path().join("lu-dev.rpack");
        std::fs::write(&region, b"first run").unwrap();
        write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            Some(42),
            Some("20260827"),
            1,
            &[("region_pack", &region)],
        )
        .unwrap();

        // A second, chargers-only run: no fresh epoch/build, only a new artifact.
        let chargers = dir.path().join("lu-dev-chargers.json");
        std::fs::write(&chargers, b"chargers").unwrap();
        write_catalog(
            &catalog_path,
            "lu-dev",
            "Luxembourg (dev)",
            None,
            None,
            2,
            &[("charger_pack", &chargers)],
        )
        .unwrap();

        let catalog: Catalog =
            serde_json::from_slice(&std::fs::read(&catalog_path).unwrap()).unwrap();
        // Inherited from the first run.
        assert_eq!(catalog.osm_snapshot_epoch, 42);
        assert_eq!(catalog.protomaps_build, "20260827");
        // Both artifacts present: the first run's entry survived the merge.
        assert!(catalog.artifacts.contains_key("region_pack"));
        assert!(catalog.artifacts.contains_key("charger_pack"));
        assert_eq!(catalog.built_at_epoch, 2);
    }
}
