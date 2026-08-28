//! Map Pack builder: shells out to the `pmtiles` CLI to cut a
//! region-clipped z14 slice of the shared Protomaps build, per ADR 0008
//! ("maxzoom 14", "one pre-merged PMTiles file per region"). Style
//! templates are vendored assets (`pipeline/assets/styles/`), copied
//! alongside the extracted tiles; the app injects the real `pmtiles://`
//! path into their `PMTILES_URL_PLACEHOLDER` source at runtime (ADR 0008
//! point 4).

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

/// Extracts `out_pmtiles` from `https://build.protomaps.com/<protomaps_build>.pmtiles`,
/// clipped to `region_geojson`, at `maxzoom`. Runs
/// `pmtiles extract <source> <out> --region=<geojson> --maxzoom=<z>`;
/// a non-zero exit is surfaced as an `io::Error` carrying the CLI's stderr.
pub fn build_map_pack(
    region_geojson: &Path,
    protomaps_build: &str,
    maxzoom: u8,
    out_pmtiles: &Path,
) -> io::Result<()> {
    if !region_geojson.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("region geojson not found: {}", region_geojson.display()),
        ));
    }
    let source_url = format!("https://build.protomaps.com/{protomaps_build}.pmtiles");
    let output = Command::new("pmtiles")
        .arg("extract")
        .arg(&source_url)
        .arg(out_pmtiles)
        .arg(format!("--region={}", region_geojson.display()))
        .arg(format!("--maxzoom={maxzoom}"))
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "pmtiles extract failed (exit {:?}): {stderr}",
            output.status.code()
        )));
    }
    Ok(())
}

/// Copies the two style templates from `styles_dir`
/// (`pipeline/assets/styles/`) into `out_dir`.
pub fn copy_styles(styles_dir: &Path, out_dir: &Path) -> io::Result<()> {
    for name in ["style-light.json", "style-dark.json"] {
        fs::copy(styles_dir.join(name), out_dir.join(name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_map_pack_errors_on_missing_geojson_without_touching_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.geo.json");
        let out = dir.path().join("out.pmtiles");
        let err = build_map_pack(&missing, "20260827", 14, &out).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn copy_styles_copies_both_templates() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("style-light.json"), b"{}").unwrap();
        fs::write(src.path().join("style-dark.json"), b"{}").unwrap();
        copy_styles(src.path(), dst.path()).unwrap();
        assert!(dst.path().join("style-light.json").exists());
        assert!(dst.path().join("style-dark.json").exists());
    }
}
