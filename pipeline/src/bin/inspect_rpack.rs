//! `inspect_rpack`: ground-truth spot check for a built `.rpack` (wayfinder
//! #65). Prints header/section info and, when the pack carries format 2.0
//! guidance, its stats and per-edge detail -- the manual QA step for a
//! guidance import before it's trusted as pipeline output.
//!
//! ```text
//! inspect_rpack <pack.rpack> [--sample N] [--near LAT,LON] [--verify]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use packs::{EdgeHot, Rpack};

struct Args {
    path: PathBuf,
    sample: Option<usize>,
    near: Option<(f32, f32)>,
    verify: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: inspect_rpack <pack.rpack> [--sample N] [--near LAT,LON] [--verify]")?;
    let mut sample = None;
    let mut near = None;
    let mut verify = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--sample" => {
                let n = args.next().ok_or("--sample requires a value")?;
                sample = Some(n.parse::<usize>().map_err(|e| format!("--sample: {e}"))?);
            }
            "--near" => {
                let v = args.next().ok_or("--near requires a value")?;
                let (lat, lon) = v.split_once(',').ok_or("--near expects LAT,LON")?;
                near = Some((
                    lat.trim()
                        .parse::<f32>()
                        .map_err(|e| format!("--near lat: {e}"))?,
                    lon.trim()
                        .parse::<f32>()
                        .map_err(|e| format!("--near lon: {e}"))?,
                ));
            }
            "--verify" => verify = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        path: PathBuf::from(path),
        sample,
        near,
        verify,
    })
}

fn human_mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// String for `id`, or `"?"` for a malformed reference -- inspection should
/// never panic on a corrupt pack.
fn string_or(pack: &Rpack, id: u32) -> String {
    if id == 0 {
        return String::new();
    }
    pack.string(id).unwrap_or("?").to_string()
}

fn describe_edge(pack: &Rpack, slot: usize, e: &EdgeHot) {
    let a = pack.nodes()[slot_from(pack, slot)];
    let b = pack.nodes()[e.target as usize];
    let attr_idx = pack
        .edge_guide()
        .get(slot)
        .copied()
        .filter(|&a| a != packs::GUIDE_NONE);
    let attr = attr_idx.and_then(|a| pack.edge_attrs().get(a as usize));
    let (name_s, ref_s) = match attr {
        Some(attr) => (string_or(pack, attr.name_id), string_or(pack, attr.ref_id)),
        None => (String::new(), String::new()),
    };
    let sign = pack.dest_sign_for_edge(slot as u32);
    println!(
        "  edge {slot}: ({:.5},{:.5}) -> ({:.5},{:.5}) len={:.0}m class={} link={} roundabout={} name={name_s:?} ref={ref_s:?}{}",
        a.lat,
        a.lon,
        b.lat,
        b.lon,
        e.length_m,
        e.guide_class(),
        e.guide_is_link(),
        e.guide_is_roundabout(),
        sign.map(|s| format!(
            " dest={:?} dest_ref={:?} junction_ref={:?}",
            string_or(pack, s.dest_id),
            string_or(pack, s.dest_ref_id),
            string_or(pack, s.junction_ref_id)
        ))
        .unwrap_or_default(),
    );
}

/// The `from` node for edge `slot`: the CSR row index is monotone
/// nondecreasing, so the owning node is the last row whose start is `<=
/// slot`. `EdgeHot` itself only carries the target, not the source.
fn slot_from(pack: &Rpack, slot: usize) -> usize {
    let csr = pack.csr_first_edge();
    csr.partition_point(|&start| start as usize <= slot) - 1
}

fn run(args: &Args) -> Result<(), String> {
    let pack = Rpack::open(&args.path).map_err(|e| format!("open {}: {e}", args.path.display()))?;

    let (major, minor) = pack.format_version();
    println!(
        "region: {} (id {}), format {major}.{minor}, osm_snapshot_epoch {}",
        pack.region_name(),
        pack.region_id(),
        pack.osm_snapshot_epoch()
    );
    println!(
        "nodes: {}, edges: {}, shortcuts: {}",
        pack.node_count(),
        pack.edges().len(),
        pack.edges()
            .iter()
            .filter(|e| e.ch_middle_node != packs::CH_MIDDLE_NODE_NONE)
            .count()
    );

    if args.verify {
        // The same deep check the install path runs (SEC-006):
        // verify_checksums pages in every section, verify_structure walks
        // every cross-section index invariant, guidance included.
        pack.verify_checksums()
            .map_err(|e| format!("checksum verification FAILED: {e}"))?;
        pack.verify_structure()
            .map_err(|e| format!("structural verification FAILED: {e}"))?;
        println!("verify: checksums OK, structure OK");
    }

    if !pack.has_guidance() {
        println!("no guidance sections (format 1.x pack)");
        return Ok(());
    }

    let n_originals = pack
        .edges()
        .iter()
        .filter(|e| e.ch_middle_node == packs::CH_MIDDLE_NODE_NONE)
        .count();
    let named_originals = pack
        .edge_guide()
        .iter()
        .zip(pack.edges())
        .filter(|(_, e)| e.ch_middle_node == packs::CH_MIDDLE_NODE_NONE)
        .filter(|(&attr, _)| {
            pack.edge_attrs()
                .get(attr as usize)
                .is_some_and(|a| a.name_id != 0)
        })
        .count();
    let named_pct = if n_originals > 0 {
        100.0 * named_originals as f64 / n_originals as f64
    } else {
        0.0
    };
    println!(
        "guidance: n_strings={}, blob={:.2}MB, n_attrs={}, dest_signs={}, exit_refs={}, named originals={named_pct:.1}%",
        pack.string_offsets().len().saturating_sub(1),
        human_mb(pack.string_blob().len() as u64),
        pack.edge_attrs().len(),
        pack.dest_signs().len(),
        pack.exit_refs().len(),
    );

    if let Some(n) = args.sample {
        let originals: Vec<(usize, &EdgeHot)> = pack
            .edges()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.ch_middle_node == packs::CH_MIDDLE_NODE_NONE)
            .collect();
        if !originals.is_empty() && n > 0 {
            let step = (originals.len() / n).max(1);
            println!("sample ({n} evenly spaced original edges):");
            for i in (0..originals.len()).step_by(step).take(n) {
                let (slot, e) = originals[i];
                describe_edge(&pack, slot, e);
            }
        }
    }

    if let Some((lat, lon)) = args.near {
        match pack.snap(lat, lon) {
            Some(node_id) => {
                println!("nearest node to ({lat},{lon}): {node_id}");
                if let Some(edges) = pack.edges_for(node_id) {
                    if let Some(range) = pack.edge_range(node_id) {
                        println!("  outgoing original edges:");
                        for (i, e) in edges.iter().enumerate() {
                            if e.ch_middle_node != packs::CH_MIDDLE_NODE_NONE {
                                continue;
                            }
                            describe_edge(&pack, range.start + i, e);
                        }
                    }
                }
                match pack.exit_ref_for_node(node_id) {
                    Some(ref_id) => println!("  exit ref: {:?}", string_or(&pack, ref_id)),
                    None => println!("  exit ref: none"),
                }
            }
            None => println!("no node found near ({lat},{lon})"),
        }
    }

    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("inspect_rpack: {e}");
            return ExitCode::FAILURE;
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inspect_rpack: {e}");
            ExitCode::FAILURE
        }
    }
}
