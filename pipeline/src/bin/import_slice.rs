//! Dev-only salvage shim: imports a throwaway prototype's raw graph slice
//! (`nodes.bin` / `edges_meta.bin` / `geometry.bin`, as left behind by the
//! `prototype/vertical-slice` branch) into a `.rpack`, contracting it along
//! the way. Real pack builds come from the OSM pipeline ticket -- this only
//! exists to unblock CH kernel testing against real-sized graphs that are
//! already sitting on disk.

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use pipeline::{build_base_model, ch_prepare, write_rpack, PackMeta};

struct Args {
    slice_dir: PathBuf,
    out: PathBuf,
    region_id: u32,
    region_name: String,
}

fn parse_args() -> Args {
    let mut slice_dir = None;
    let mut out = None;
    let mut region_id = None;
    let mut region_name = None;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut next_value = || {
            args.next()
                .unwrap_or_else(|| panic!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--slice-dir" => slice_dir = Some(PathBuf::from(next_value())),
            "--out" => out = Some(PathBuf::from(next_value())),
            "--region-id" => {
                region_id = Some(next_value().parse().expect("--region-id must be a u32"))
            }
            "--region-name" => region_name = Some(next_value()),
            other => panic!("unknown argument: {other}"),
        }
    }

    Args {
        slice_dir: slice_dir.expect("--slice-dir is required"),
        out: out.expect("--out is required"),
        region_id: region_id.expect("--region-id is required"),
        region_name: region_name.expect("--region-name is required"),
    }
}

fn main() {
    let args = parse_args();

    println!("reading slice from {}", args.slice_dir.display());
    let t0 = Instant::now();
    let (base, stats) =
        build_base_model(&args.slice_dir).expect("failed to build base model from slice");
    println!(
        "loaded {} nodes, {} edges, {} geometry points in {:?} ({} edges had speed_kmh <= 0, clamped to {:.1})",
        stats.n_nodes,
        stats.n_edges,
        stats.n_geometry_points,
        t0.elapsed(),
        stats.speed_clamped,
        5.0,
    );

    let t1 = Instant::now();
    let (contracted, ch_stats) = ch_prepare(&base);
    println!(
        "ch_prepare: {} shortcuts added, max_settled={}, took {:?}",
        ch_stats.shortcuts_added,
        ch_stats.max_settled,
        t1.elapsed()
    );

    let meta = PackMeta {
        osm_snapshot_epoch: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        region_id: args.region_id,
        region_name: args.region_name,
    };

    let t2 = Instant::now();
    write_rpack(&contracted, &meta, &args.out).expect("failed to write rpack");
    let elapsed = t2.elapsed();
    let size = std::fs::metadata(&args.out)
        .expect("output file should exist after write_rpack")
        .len();
    println!("wrote {} ({size} bytes) in {elapsed:?}", args.out.display());
}
