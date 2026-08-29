//! `build_packs`: the one command from raw OSM/Charger sources to
//! installable Region Pack + Charger Pack + Map Pack artifacts for one
//! region, per wayfinder ticket #35 (ADR 0005, 0007, 0008).
//!
//! ```text
//! build_packs --region lu-dev|corridor|eu-west [--sources DIR=~/abrp-data] [--out DIR]
//!             [--jobs region,chargers,map] [--protomaps-build 20260827]
//!             [--dem-cache DIR=<sources>/dem] [--allow-partial]
//! ```
//!
//! The `chargers` job fails closed (wayfinder ticket #H-04): every feed a
//! region declares is required, and the graph bbox used to geographically
//! filter chargers (this run's own `region` job, or else an existing
//! `.rpack`) must be available. A missing/malformed/zero-result feed or a
//! missing Region Pack fails the build with a clear error naming what's
//! missing, unless `--allow-partial` is passed, which restores the old
//! degrade-to-warning behavior for an intentionally partial build.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use packs::{NodeRecord, Rpack};
use pipeline::{
    build_map_pack, ch_prepare, chargers, copy_styles, filter_bbox, import_pbfs,
    write_charger_pack, write_rpack, PackMeta,
};

/// Bbox margin (degrees) applied around the graph's own bbox when filtering
/// chargers, so a charger just outside the region's roads (border extract
/// clipping) still shows up.
const CHARGER_BBOX_MARGIN_DEG: f64 = 0.2;

const MAP_MAXZOOM: u8 = 14;

struct Args {
    region: String,
    sources: PathBuf,
    out: PathBuf,
    jobs: Vec<String>,
    protomaps_build: String,
    dem_cache: PathBuf,
    /// Restores the pre-H-04 degrade-to-warning behavior: a required
    /// charger feed or Region Pack that can't be satisfied contributes zero
    /// records / disables bbox filtering instead of failing the build.
    allow_partial: bool,
}

fn parse_args() -> Args {
    let mut region = None;
    let mut sources = None;
    let mut out = None;
    let mut jobs = None;
    let mut protomaps_build = None;
    let mut dem_cache = None;
    let mut allow_partial = false;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut next_value = || {
            args.next()
                .unwrap_or_else(|| panic!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--region" => region = Some(next_value()),
            "--sources" => sources = Some(PathBuf::from(next_value())),
            "--out" => out = Some(PathBuf::from(next_value())),
            "--jobs" => jobs = Some(next_value().split(',').map(str::to_string).collect()),
            "--protomaps-build" => protomaps_build = Some(next_value()),
            "--dem-cache" => dem_cache = Some(PathBuf::from(next_value())),
            "--allow-partial" => allow_partial = true,
            other => panic!("unknown argument: {other}"),
        }
    }

    let sources = sources.unwrap_or_else(|| {
        let home = std::env::var("HOME").expect("HOME is not set; pass --sources explicitly");
        PathBuf::from(home).join("abrp-data")
    });
    let dem_cache = dem_cache.unwrap_or_else(|| sources.join("dem"));

    Args {
        region: region.expect("--region is required (lu-dev, corridor, or eu-west)"),
        out: out.unwrap_or_else(|| PathBuf::from("out")),
        jobs: jobs.unwrap_or_else(|| {
            ["region", "chargers", "map"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        }),
        protomaps_build: protomaps_build.unwrap_or_else(|| "20260827".to_string()),
        dem_cache,
        sources,
        allow_partial,
    }
}

/// A national charger feed `run_chargers_job` knows how to parse. A region
/// opts into the feeds whose country it actually covers.
#[derive(PartialEq, Eq)]
enum ChargerFeed {
    Ndw,
    RoadBe,
    ChargyKml,
    IrveFr,
    Bnetza,
}

/// Built-in region registry: which PBFs make up each region, its display
/// name, its `.rpack` header id, and which charger feeds it draws from.
struct RegionSpec {
    pbfs: &'static [&'static str],
    region_name: &'static str,
    region_numeric_id: u32,
    charger_feeds: &'static [ChargerFeed],
}

fn region_spec(region: &str) -> RegionSpec {
    const BENELUX_FEEDS: &[ChargerFeed] = &[
        ChargerFeed::Ndw,
        ChargerFeed::RoadBe,
        ChargerFeed::ChargyKml,
    ];
    match region {
        "lu-dev" => RegionSpec {
            pbfs: &["luxembourg-latest.osm.pbf"],
            region_name: "Luxembourg (dev)",
            region_numeric_id: 1,
            charger_feeds: BENELUX_FEEDS,
        },
        "corridor" => RegionSpec {
            pbfs: &[
                "luxembourg-latest.osm.pbf",
                "belgium-latest.osm.pbf",
                "netherlands-latest.osm.pbf",
            ],
            region_name: "LU+BE+NL corridor",
            region_numeric_id: 2,
            charger_feeds: BENELUX_FEEDS,
        },
        "eu-west" => RegionSpec {
            pbfs: &[
                "luxembourg-latest.osm.pbf",
                "belgium-latest.osm.pbf",
                "netherlands-latest.osm.pbf",
                "france-latest.osm.pbf",
                "germany-latest.osm.pbf",
            ],
            region_name: "Benelux+FR+DE",
            region_numeric_id: 3,
            charger_feeds: &[
                ChargerFeed::Ndw,
                ChargerFeed::RoadBe,
                ChargerFeed::ChargyKml,
                ChargerFeed::IrveFr,
                ChargerFeed::Bnetza,
            ],
        },
        other => panic!("unknown region {other:?}; known regions: lu-dev, corridor, eu-west"),
    }
}

/// (min_lat, min_lon, max_lat, max_lon) over a node slice.
fn bbox_of(nodes: &[NodeRecord]) -> (f64, f64, f64, f64) {
    let mut min_lat = f32::INFINITY;
    let mut max_lat = f32::NEG_INFINITY;
    let mut min_lon = f32::INFINITY;
    let mut max_lon = f32::NEG_INFINITY;
    for n in nodes {
        min_lat = min_lat.min(n.lat);
        max_lat = max_lat.max(n.lat);
        min_lon = min_lon.min(n.lon);
        max_lon = max_lon.max(n.lon);
    }
    (
        min_lat as f64,
        min_lon as f64,
        max_lat as f64,
        max_lon as f64,
    )
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs()
}

/// Runs the `region` job: OSM import -> (elevation, wired at merge) ->
/// CH contraction -> `.rpack`. Returns the pack path, its bbox (for the
/// `chargers` job in the same run), and the epoch stamped into it.
fn run_region_job(args: &Args, spec: &RegionSpec) -> (PathBuf, (f64, f64, f64, f64), u64) {
    let pbf_paths: Vec<PathBuf> = spec.pbfs.iter().map(|f| args.sources.join(f)).collect();
    println!(
        "[build_packs] region: importing {} PBF(s) for {}",
        pbf_paths.len(),
        args.region
    );
    let t0 = Instant::now();
    let (mut model, stats) = import_pbfs(&pbf_paths).expect("osm import failed");
    println!(
        "[build_packs] region: {} ways kept, {} junction nodes, {} edges, {} nodes dropped by SCC prune, took {:?}",
        stats.ways_kept, stats.junction_nodes, stats.edges, stats.dropped_scc_nodes, t0.elapsed()
    );

    let epoch = stats
        .file_epochs
        .iter()
        .filter_map(|(_, e)| *e)
        .min()
        .unwrap_or_else(|| {
            println!(
                "[build_packs] region: warning: no osm_snapshot_epoch found in any input PBF, defaulting to 0"
            );
            0
        });

    let bbox = bbox_of(&model.nodes);

    let t_elev = Instant::now();
    let elev_stats =
        pipeline::apply_elevation(&mut model, &args.dem_cache).expect("elevation sampling failed");
    println!(
        "[build_packs] region: elevation: {} vertices over {} edges (tiles: {} fetched, {} cached, {} ocean), range {}..{} m, took {:?}",
        elev_stats.vertices_sampled,
        elev_stats.edges_updated,
        elev_stats.tiles_fetched,
        elev_stats.tiles_cache_hits,
        elev_stats.tiles_missing,
        elev_stats.min_elev_m,
        elev_stats.max_elev_m,
        t_elev.elapsed()
    );

    let t1 = Instant::now();
    let (contracted, ch_stats) = ch_prepare(&model);
    println!(
        "[build_packs] region: ch_prepare: {} shortcuts, max_settled={}, took {:?}",
        ch_stats.shortcuts_added,
        ch_stats.max_settled,
        t1.elapsed()
    );

    let rpack_path = args.out.join(format!("{}.rpack", args.region));
    let meta = PackMeta {
        osm_snapshot_epoch: epoch,
        region_id: spec.region_numeric_id,
        region_name: spec.region_name.to_string(),
    };
    write_rpack(&contracted, &meta, &rpack_path).expect("failed to write rpack");
    println!("[build_packs] region: wrote {}", rpack_path.display());

    (rpack_path, bbox, epoch)
}

/// One feed's raw parse result, named for `require_all_feeds`'s fail-closed
/// decision: `Err` covers both a parse/IO failure and a successful parse
/// that yielded zero records, since a required national feed silently
/// going empty (schema drift, wrong path) is exactly what a fail-closed
/// build must catch.
type FeedOutcome = (&'static str, Result<Vec<chargers::ChargerRecord>, String>);

/// Wraps one feed's raw parse result as a `FeedOutcome`, naming the feed
/// and its source path in the error message.
fn feed_outcome(
    feed_name: &'static str,
    path_desc: &str,
    parsed: io::Result<Vec<chargers::ChargerRecord>>,
) -> FeedOutcome {
    let result = match parsed {
        Ok(records) if records.is_empty() => Err(format!(
            "{feed_name} feed at {path_desc} parsed but yielded zero charger records"
        )),
        Ok(records) => Ok(records),
        Err(e) => Err(format!("{feed_name} feed at {path_desc}: {e}")),
    };
    (feed_name, result)
}

/// One feed's resolved records for this build, plus (if it didn't satisfy
/// the "required feed" contract) the reason it was degraded to zero
/// records under `--allow-partial`.
#[derive(Debug)]
struct FeedReport {
    name: &'static str,
    records: Vec<chargers::ChargerRecord>,
    degraded: Option<String>,
}

/// Resolves every feed's `FeedOutcome` into the records to use for this
/// build. Fails closed (`Err`, naming every failed feed) unless
/// `allow_partial`, in which case a failed feed degrades to zero records
/// instead of failing the whole job. Always returns one report per input
/// feed, including a degraded one, so the caller can log parsed/kept
/// counts uniformly either way.
fn require_all_feeds(
    outcomes: Vec<FeedOutcome>,
    allow_partial: bool,
) -> Result<Vec<FeedReport>, String> {
    let failures: Vec<&str> = outcomes
        .iter()
        .filter_map(|(_, r)| r.as_ref().err().map(String::as_str))
        .collect();
    if !failures.is_empty() && !allow_partial {
        return Err(format!(
            "required charger feed(s) failed (pass --allow-partial to degrade instead): {}",
            failures.join("; ")
        ));
    }
    Ok(outcomes
        .into_iter()
        .map(|(name, r)| match r {
            Ok(records) => FeedReport {
                name,
                records,
                degraded: None,
            },
            Err(reason) => FeedReport {
                name,
                records: Vec::new(),
                degraded: Some(reason),
            },
        })
        .collect())
}

/// Resolves the bbox used to clip charger records to the region: the bbox
/// from this session's own `region` job if it ran, else read back from an
/// existing `.rpack` at `rpack_path`. Fails closed (`Err`) when neither is
/// available, unless `allow_partial`, in which case the charger list is
/// left unfiltered (`Ok(None)`) -- the historical behavior.
fn resolve_region_bbox(
    region_bbox: Option<(f64, f64, f64, f64)>,
    rpack_path: &Path,
    allow_partial: bool,
) -> Result<Option<(f64, f64, f64, f64)>, String> {
    if let Some(bbox) = region_bbox {
        return Ok(Some(bbox));
    }
    match Rpack::open(rpack_path) {
        Ok(rpack) => Ok(Some(bbox_of(rpack.nodes()))),
        Err(e) => {
            let msg = format!(
                "no region pack at {} ({e}); charger list cannot be geographically filtered",
                rpack_path.display()
            );
            if allow_partial {
                println!(
                    "[build_packs] chargers: WARNING: {msg}; continuing unfiltered (--allow-partial)"
                );
                Ok(None)
            } else {
                Err(msg)
            }
        }
    }
}

/// Runs the `chargers` job: parses the region's listed feeds (`spec.
/// charger_feeds` -- the charger bbox clip below is a bounding rectangle,
/// not a polygon, so a region must opt into feeds, or corridor's rectangle
/// sweeps in FR/DE stations its road graph has no roads for, which would
/// then snap onto Benelux roads as phantom candidates), clips to the graph
/// bbox (from this session's `region` job if it ran, else read back from
/// an existing `.rpack`), writes the Charger Pack.
///
/// Every declared feed and the region bbox are required (wayfinder H-04):
/// a missing/malformed/zero-result feed, or no way to determine the bbox,
/// fails the job with `Err` naming what's missing -- unless `allow_partial`
/// is set on `args`, which degrades the failing input to empty/unfiltered
/// instead, after printing a prominent warning. Per-feed parsed/kept
/// counts are always logged.
fn run_chargers_job(
    args: &Args,
    spec: &RegionSpec,
    region_bbox: Option<(f64, f64, f64, f64)>,
) -> Result<PathBuf, String> {
    let feeds = spec.charger_feeds;
    let mut outcomes: Vec<FeedOutcome> = Vec::new();

    if feeds.contains(&ChargerFeed::Ndw) {
        let path = args.sources.join("ndw_chargers.json.gz");
        let parsed = chargers::parse_ndw_gz(&path);
        outcomes.push(feed_outcome("ndw", &path.display().to_string(), parsed));
    }
    if feeds.contains(&ChargerFeed::RoadBe) {
        let path = args.sources.join("road_chargers.json");
        let parsed = chargers::parse_roadbe(&path);
        outcomes.push(feed_outcome("roadbe", &path.display().to_string(), parsed));
    }
    if feeds.contains(&ChargerFeed::ChargyKml) {
        let path = args.sources.join("chargy.kml");
        let parsed = chargers::parse_chargy_kml(&path);
        outcomes.push(feed_outcome("chargy", &path.display().to_string(), parsed));
    }
    if feeds.contains(&ChargerFeed::IrveFr) {
        let path = args.sources.join("irve_fr.csv");
        let parsed = chargers::parse_irve_fr(&path);
        outcomes.push(feed_outcome("irve", &path.display().to_string(), parsed));
    }
    if feeds.contains(&ChargerFeed::Bnetza) {
        let ladestation_path = args.sources.join("bnetza_api_ladestation000.csv");
        let ladepunkt_path = args.sources.join("bnetza_api_ladepunkt000.csv");
        let stecker_path = args.sources.join("bnetza_api_stecker000.csv");
        let parsed = chargers::parse_bnetza(&ladestation_path, &ladepunkt_path, &stecker_path);
        let path_desc = format!(
            "{}, {}, {}",
            ladestation_path.display(),
            ladepunkt_path.display(),
            stecker_path.display()
        );
        outcomes.push(feed_outcome("bnetza", &path_desc, parsed));
    }

    let feed_reports = require_all_feeds(outcomes, args.allow_partial)?;
    if feed_reports.iter().any(|r| r.degraded.is_some()) {
        println!(
            "[build_packs] chargers: WARNING: proceeding with degraded feed(s) (--allow-partial):"
        );
        for r in &feed_reports {
            if let Some(reason) = &r.degraded {
                println!("[build_packs] chargers:   - {reason}");
            }
        }
    }

    let rpack_path = args.out.join(format!("{}.rpack", args.region));
    let bbox = resolve_region_bbox(region_bbox, &rpack_path, args.allow_partial)?;

    let mut records = Vec::new();
    for report in feed_reports {
        let parsed = report.records.len();
        let kept = match bbox {
            Some((min_lat, min_lon, max_lat, max_lon)) => filter_bbox(
                report.records,
                min_lat - CHARGER_BBOX_MARGIN_DEG,
                min_lon - CHARGER_BBOX_MARGIN_DEG,
                max_lat + CHARGER_BBOX_MARGIN_DEG,
                max_lon + CHARGER_BBOX_MARGIN_DEG,
            ),
            None => report.records,
        };
        println!(
            "[build_packs] chargers: {}: parsed {parsed}, kept {}",
            report.name,
            kept.len()
        );
        records.extend(kept);
    }
    println!(
        "[build_packs] chargers: {} kept after bbox filter (total)",
        records.len()
    );

    let chargers_path = args.out.join(format!("{}-chargers.json", args.region));
    write_charger_pack(&chargers_path, &args.region, &records)
        .map_err(|e| format!("failed to write charger pack: {e}"))?;
    println!("[build_packs] chargers: wrote {}", chargers_path.display());

    Ok(chargers_path)
}

/// Runs the `map` job: extracts a region-clipped z14 PMTiles slice from the
/// shared Protomaps build and copies the style templates alongside it.
fn run_map_job(args: &Args) -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let region_geojson = crate_dir
        .join("assets/regions")
        .join(format!("{}.geo.json", args.region));
    let out_pmtiles = args.out.join(format!("{}.pmtiles", args.region));

    println!(
        "[build_packs] map: extracting {} at maxzoom {MAP_MAXZOOM} from build {}",
        region_geojson.display(),
        args.protomaps_build
    );
    build_map_pack(
        &region_geojson,
        &args.protomaps_build,
        MAP_MAXZOOM,
        &out_pmtiles,
    )
    .expect("map pack build failed");
    println!("[build_packs] map: wrote {}", out_pmtiles.display());

    let styles_dir = crate_dir.join("assets/styles");
    copy_styles(&styles_dir, &args.out).expect("failed to copy style templates");
    println!(
        "[build_packs] map: copied style templates to {}",
        args.out.display()
    );

    out_pmtiles
}

fn main() {
    let args = parse_args();
    let spec = region_spec(&args.region);
    std::fs::create_dir_all(&args.out).expect("failed to create --out dir");

    let mut osm_snapshot_epoch: Option<u64> = None;
    let mut region_bbox: Option<(f64, f64, f64, f64)> = None;
    let mut new_artifacts: Vec<(&str, PathBuf)> = Vec::new();

    if args.jobs.iter().any(|j| j == "region") {
        let (path, bbox, epoch) = run_region_job(&args, &spec);
        region_bbox = Some(bbox);
        osm_snapshot_epoch = Some(epoch);
        new_artifacts.push(("region_pack", path));
    }

    if args.jobs.iter().any(|j| j == "chargers") {
        let path = run_chargers_job(&args, &spec, region_bbox).unwrap_or_else(|e| {
            eprintln!("[build_packs] chargers: FAILED: {e}");
            std::process::exit(1);
        });
        new_artifacts.push(("charger_pack", path));
    }

    if args.jobs.iter().any(|j| j == "map") {
        let path = run_map_job(&args);
        new_artifacts.push(("map_pack", path));
    }

    let protomaps_build_for_catalog = args
        .jobs
        .iter()
        .any(|j| j == "map")
        .then_some(args.protomaps_build.as_str());
    let artifact_refs: Vec<(&str, &Path)> = new_artifacts
        .iter()
        .map(|(name, path)| (*name, path.as_path()))
        .collect();

    let catalog_path = args.out.join("catalog.json");
    pipeline::catalog::write_catalog(
        &catalog_path,
        &args.region,
        spec.region_name,
        osm_snapshot_epoch,
        protomaps_build_for_catalog,
        now_epoch(),
        &artifact_refs,
    )
    .unwrap_or_else(|e| {
        eprintln!("[build_packs] catalog: FAILED: {e}");
        std::process::exit(1);
    });
    println!("[build_packs] wrote {}", catalog_path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn lu_dev_spec_with(feeds: &'static [ChargerFeed]) -> RegionSpec {
        RegionSpec {
            pbfs: &[],
            region_name: "Luxembourg (dev)",
            region_numeric_id: 1,
            charger_feeds: feeds,
        }
    }

    fn args_for(sources: PathBuf, out: PathBuf, allow_partial: bool) -> Args {
        Args {
            region: "lu-dev".to_string(),
            sources,
            out,
            jobs: vec!["chargers".to_string()],
            protomaps_build: "20260827".to_string(),
            dem_cache: PathBuf::from("unused"),
            allow_partial,
        }
    }

    fn one_record() -> chargers::ChargerRecord {
        chargers::ChargerRecord {
            id: "x".into(),
            name: "x".into(),
            lat: 49.5,
            lon: 6.1,
            operator: None,
            access: None,
            country: "LU".into(),
            max_power_kw: 150.0,
            connectors: vec![],
            source: "test".into(),
        }
    }

    // --- feed_outcome / require_all_feeds (pure, no I/O) ---

    #[test]
    fn feed_outcome_fails_on_zero_records() {
        let (name, result) = feed_outcome("ndw", "some/path", Ok(Vec::new()));
        assert_eq!(name, "ndw");
        assert!(result.unwrap_err().contains("zero charger records"));
    }

    #[test]
    fn feed_outcome_fails_on_io_error() {
        let err = io::Error::new(io::ErrorKind::NotFound, "no such file");
        let (_, result) = feed_outcome("ndw", "some/path", Err(err));
        let msg = result.unwrap_err();
        assert!(msg.contains("ndw"));
        assert!(msg.contains("some/path"));
    }

    #[test]
    fn require_all_feeds_fails_closed_on_any_failure_without_allow_partial() {
        let outcomes: Vec<FeedOutcome> = vec![
            ("ndw", Ok(vec![one_record()])),
            ("roadbe", Err("roadbe feed at x: boom".to_string())),
        ];
        let err = require_all_feeds(outcomes, false).unwrap_err();
        assert!(err.contains("roadbe"));
    }

    #[test]
    fn require_all_feeds_degrades_failed_feed_under_allow_partial() {
        let outcomes: Vec<FeedOutcome> = vec![
            ("ndw", Ok(vec![one_record()])),
            ("roadbe", Err("roadbe feed at x: boom".to_string())),
        ];
        let reports = require_all_feeds(outcomes, true).unwrap();
        let ndw = reports.iter().find(|r| r.name == "ndw").unwrap();
        assert!(ndw.degraded.is_none());
        assert_eq!(ndw.records.len(), 1);
        let roadbe = reports.iter().find(|r| r.name == "roadbe").unwrap();
        assert!(roadbe.degraded.is_some());
        assert!(roadbe.records.is_empty());
    }

    // --- resolve_region_bbox ---

    #[test]
    fn resolve_region_bbox_fails_closed_when_rpack_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.rpack");
        let err = resolve_region_bbox(None, &missing, false).unwrap_err();
        assert!(err.contains("no region pack"));
    }

    #[test]
    fn resolve_region_bbox_degrades_to_unfiltered_under_allow_partial() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.rpack");
        let bbox = resolve_region_bbox(None, &missing, true).unwrap();
        assert!(bbox.is_none());
    }

    #[test]
    fn resolve_region_bbox_prefers_the_session_bbox_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.rpack");
        let bbox = resolve_region_bbox(Some((1.0, 2.0, 3.0, 4.0)), &missing, false).unwrap();
        assert_eq!(bbox, Some((1.0, 2.0, 3.0, 4.0)));
    }

    // --- run_chargers_job: end-to-end fail-closed / degrade behavior ---
    // Follows the reference test matrix from the audit: missing feed file,
    // malformed feed, zero-result feed, missing Region Pack -- each fails
    // without `--allow-partial` and degrades to an empty/unfiltered result
    // with it.

    const SUPERCHARGY_KML: &str = "<kml><Document><Placemark><name>SuperChargy Test</name><Point><coordinates>6.1,49.5,0</coordinates></Point></Placemark></Document></kml>";
    const REGULAR_CHARGY_KML: &str = "<kml><Document><Placemark><name>Chargy Regular</name><Point><coordinates>6.1,49.5,0</coordinates></Point></Placemark></Document></kml>";

    #[test]
    fn missing_feed_file_fails_without_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(
            sources.path().to_path_buf(),
            out.path().to_path_buf(),
            false,
        );
        // No chargy.kml written at all.
        let err = run_chargers_job(&args, &spec, Some((49.0, 6.0, 50.0, 7.0))).unwrap_err();
        assert!(err.contains("chargy"));
    }

    #[test]
    fn missing_feed_file_degrades_with_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(sources.path().to_path_buf(), out.path().to_path_buf(), true);
        let path = run_chargers_job(&args, &spec, Some((49.0, 6.0, 50.0, 7.0))).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(doc["charger_count"], 0);
    }

    #[test]
    fn malformed_feed_fails_without_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(sources.path().join("chargy.kml"), [0xFFu8, 0xFE, 0xFD]).unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(
            sources.path().to_path_buf(),
            out.path().to_path_buf(),
            false,
        );
        let err = run_chargers_job(&args, &spec, Some((49.0, 6.0, 50.0, 7.0))).unwrap_err();
        assert!(err.contains("chargy"));
    }

    #[test]
    fn malformed_feed_degrades_with_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(sources.path().join("chargy.kml"), [0xFFu8, 0xFE, 0xFD]).unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(sources.path().to_path_buf(), out.path().to_path_buf(), true);
        let path = run_chargers_job(&args, &spec, Some((49.0, 6.0, 50.0, 7.0))).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(doc["charger_count"], 0);
    }

    #[test]
    fn zero_result_feed_fails_without_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(sources.path().join("chargy.kml"), REGULAR_CHARGY_KML).unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(
            sources.path().to_path_buf(),
            out.path().to_path_buf(),
            false,
        );
        let err = run_chargers_job(&args, &spec, Some((49.0, 6.0, 50.0, 7.0))).unwrap_err();
        assert!(err.contains("zero charger records"));
    }

    #[test]
    fn zero_result_feed_degrades_with_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(sources.path().join("chargy.kml"), REGULAR_CHARGY_KML).unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(sources.path().to_path_buf(), out.path().to_path_buf(), true);
        let path = run_chargers_job(&args, &spec, Some((49.0, 6.0, 50.0, 7.0))).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(doc["charger_count"], 0);
    }

    #[test]
    fn missing_region_pack_fails_without_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(sources.path().join("chargy.kml"), SUPERCHARGY_KML).unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(
            sources.path().to_path_buf(),
            out.path().to_path_buf(),
            false,
        );
        // region_bbox is None (no `region` job ran this session) and no
        // lu-dev.rpack exists in `out` either.
        let err = run_chargers_job(&args, &spec, None).unwrap_err();
        assert!(err.contains("no region pack"));
    }

    #[test]
    fn missing_region_pack_degrades_with_allow_partial() {
        let sources = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(sources.path().join("chargy.kml"), SUPERCHARGY_KML).unwrap();
        let spec = lu_dev_spec_with(&[ChargerFeed::ChargyKml]);
        let args = args_for(sources.path().to_path_buf(), out.path().to_path_buf(), true);
        let path = run_chargers_job(&args, &spec, None).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        // Unfiltered: the one parsed record is kept regardless of bbox.
        assert_eq!(doc["charger_count"], 1);
    }
}
