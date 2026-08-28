//! `build_packs`: the one command from raw OSM/Charger sources to
//! installable Region Pack + Charger Pack + Map Pack artifacts for one
//! region, per wayfinder ticket #35 (ADR 0005, 0007, 0008).
//!
//! ```text
//! build_packs --region lu-dev|corridor [--sources DIR=~/abrp-data] [--out DIR]
//!             [--jobs region,chargers,map] [--protomaps-build 20260827]
//!             [--dem-cache DIR=<sources>/dem]
//! ```

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
}

fn parse_args() -> Args {
    let mut region = None;
    let mut sources = None;
    let mut out = None;
    let mut jobs = None;
    let mut protomaps_build = None;
    let mut dem_cache = None;

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
            other => panic!("unknown argument: {other}"),
        }
    }

    let sources = sources.unwrap_or_else(|| {
        let home = std::env::var("HOME").expect("HOME is not set; pass --sources explicitly");
        PathBuf::from(home).join("abrp-data")
    });
    let dem_cache = dem_cache.unwrap_or_else(|| sources.join("dem"));

    Args {
        region: region.expect("--region is required (lu-dev or corridor)"),
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
    }
}

/// Built-in region registry: which PBFs make up each region, its display
/// name, and its `.rpack` header id.
struct RegionSpec {
    pbfs: &'static [&'static str],
    region_name: &'static str,
    region_numeric_id: u32,
}

fn region_spec(region: &str) -> RegionSpec {
    match region {
        "lu-dev" => RegionSpec {
            pbfs: &["luxembourg-latest.osm.pbf"],
            region_name: "Luxembourg (dev)",
            region_numeric_id: 1,
        },
        "corridor" => RegionSpec {
            pbfs: &[
                "luxembourg-latest.osm.pbf",
                "belgium-latest.osm.pbf",
                "netherlands-latest.osm.pbf",
            ],
            region_name: "LU+BE+NL corridor",
            region_numeric_id: 2,
        },
        other => panic!("unknown region {other:?}; known regions: lu-dev, corridor"),
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

/// Runs the `chargers` job: parses all three feeds, clips to the graph
/// bbox (from this session's `region` job if it ran, else read back from
/// an existing `.rpack`, else unfiltered with a warning), writes the
/// Charger Pack.
fn run_chargers_job(args: &Args, region_bbox: Option<(f64, f64, f64, f64)>) -> PathBuf {
    let mut records = Vec::new();

    let ndw_path = args.sources.join("ndw_chargers.json.gz");
    match chargers::parse_ndw_gz(&ndw_path) {
        Ok(mut v) => records.append(&mut v),
        Err(e) => println!(
            "[build_packs] chargers: warning: {}: {e}",
            ndw_path.display()
        ),
    }
    let road_path = args.sources.join("road_chargers.json");
    match chargers::parse_roadbe(&road_path) {
        Ok(mut v) => records.append(&mut v),
        Err(e) => println!(
            "[build_packs] chargers: warning: {}: {e}",
            road_path.display()
        ),
    }
    let chargy_path = args.sources.join("chargy.kml");
    match chargers::parse_chargy_kml(&chargy_path) {
        Ok(mut v) => records.append(&mut v),
        Err(e) => println!(
            "[build_packs] chargers: warning: {}: {e}",
            chargy_path.display()
        ),
    }
    println!(
        "[build_packs] chargers: parsed {} candidate locations",
        records.len()
    );

    let bbox = region_bbox.or_else(|| {
        let rpack_path = args.out.join(format!("{}.rpack", args.region));
        match Rpack::open(&rpack_path) {
            Ok(rpack) => Some(bbox_of(rpack.nodes())),
            Err(e) => {
                println!(
                    "[build_packs] chargers: warning: no region pack at {} ({e}); charger list will be unfiltered",
                    rpack_path.display()
                );
                None
            }
        }
    });

    let records = match bbox {
        Some((min_lat, min_lon, max_lat, max_lon)) => filter_bbox(
            records,
            min_lat - CHARGER_BBOX_MARGIN_DEG,
            min_lon - CHARGER_BBOX_MARGIN_DEG,
            max_lat + CHARGER_BBOX_MARGIN_DEG,
            max_lon + CHARGER_BBOX_MARGIN_DEG,
        ),
        None => records,
    };
    println!(
        "[build_packs] chargers: {} kept after bbox filter",
        records.len()
    );

    let chargers_path = args.out.join(format!("{}-chargers.json", args.region));
    write_charger_pack(&chargers_path, &args.region, &records)
        .expect("failed to write charger pack");
    println!("[build_packs] chargers: wrote {}", chargers_path.display());

    chargers_path
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
        let path = run_chargers_job(&args, region_bbox);
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
    .expect("failed to write catalog.json");
    println!("[build_packs] wrote {}", catalog_path.display());
}
