//! Synthetic-junction unit tests for the maneuver classifier (wayfinder
//! #66). Each test builds a tiny format-2.0 pack via `common::build_pack`
//! (nodes placed with `common::behind`/`common::ahead` so entry/exit
//! bearings at the junction under test are exactly engineered) and asserts
//! the full step list `guidance::steps_for_route` produces.

mod common;

use common::{ahead, behind, open_pack, EdgeSpec};
use guidance::{steps_for_route, ManeuverModifier, ManeuverType};
use packs::{
    EdgeHot, GeomVertex, NodeRecord, RegionGraphModel, Rpack, SnapGridModel, CH_MIDDLE_NODE_NONE,
    GUIDE_CLASS_MOTORWAY, GUIDE_CLASS_PRIMARY, GUIDE_CLASS_RESIDENTIAL,
};
use pipeline::{write_rpack, PackMeta};

fn f32pair(p: (f64, f64)) -> (f32, f32) {
    (p.0 as f32, p.1 as f32)
}

/// Case 1: Right-angle turn with a straight alternative -> Turn/Right.
#[test]
fn right_angle_turn_with_alternative_emits_turn_right() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 90.0, 300.0); // cur: right turn
    let d = ahead(j, 0.0, 200.0); // alt: straight continuation

    let e0 = EdgeSpec {
        name: "Rue A",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "Rue B",
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        name: "Rue A2",
        ..EdgeSpec::new(1, 3, 200.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].maneuver, ManeuverType::Depart);
    assert_eq!(steps[0].name, "Rue A");
    assert_eq!(steps[0].dist_from_leg_start_m, 0.0);
    assert_eq!(steps[1].maneuver, ManeuverType::Turn);
    assert_eq!(steps[1].modifier, ManeuverModifier::Right);
    assert_eq!(steps[1].name, "Rue B");
    assert_eq!(steps[1].dist_from_leg_start_m, 500.0);
    assert_eq!(steps[2].maneuver, ManeuverType::Arrive);
    assert_eq!(steps[2].dist_from_leg_start_m, 800.0);
}

/// Case 2: Crossroads, straight through, same name -> no interior step
/// (suppressed by rule 6d: plain straight continuation).
#[test]
fn crossroads_straight_through_same_name_is_suppressed() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 0.0, 400.0);
    let d = ahead(j, 90.0, 200.0); // side alternative

    let e0 = EdgeSpec {
        name: "Main St",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "Main St",
        ..EdgeSpec::new(1, 2, 400.0)
    };
    let alt = EdgeSpec {
        name: "Side St",
        ..EdgeSpec::new(1, 3, 200.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].maneuver, ManeuverType::Depart);
    assert_eq!(steps[1].maneuver, ManeuverType::Arrive);
}

/// Case 3: Straight through with a name change at the junction -> Continue.
#[test]
fn straight_through_name_change_emits_continue() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 0.0, 400.0);

    let e0 = EdgeSpec {
        name: "Main St",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "Second St",
        ..EdgeSpec::new(1, 2, 400.0)
    };
    let nodes = [a, b, c];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::Continue);
    assert_eq!(steps[1].modifier, ManeuverModifier::Straight);
    assert_eq!(steps[1].name, "Second St");
    assert_eq!(steps[1].dist_from_leg_start_m, 500.0);
}

/// Case 4: Sharp bend, NO alternatives -> no interior step (no decision to make).
#[test]
fn sharp_bend_with_no_alternatives_emits_nothing() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 150.0, 300.0); // sharp bend, dev=150

    let e0 = EdgeSpec {
        name: "Rue A",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "Rue A",
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let nodes = [a, b, c];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 2);
}

/// Case 5a: T-junction (end of road), route takes the right branch.
#[test]
fn end_of_road_emits_right() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 90.0, 300.0); // cur: east
    let d = ahead(j, 270.0, 300.0); // alt: west

    let e0 = EdgeSpec {
        name: "Approach Rd",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "East Rd",
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        name: "West Rd",
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::EndOfRoad);
    assert_eq!(steps[1].modifier, ManeuverModifier::Right);
    assert_eq!(steps[1].name, "East Rd");
}

/// Case 5b: T-junction (end of road), route takes the left branch.
#[test]
fn end_of_road_emits_left() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 270.0, 300.0); // cur: west
    let d = ahead(j, 90.0, 300.0); // alt: east

    let e0 = EdgeSpec {
        name: "Approach Rd",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "West Rd",
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        name: "East Rd",
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::EndOfRoad);
    assert_eq!(steps[1].modifier, ManeuverModifier::Left);
    assert_eq!(steps[1].name, "West Rd");
}

/// Case 6: Near-straight fork, different names, same class -> Fork/Slight.
#[test]
fn near_straight_fork_emits_fork_slight_right() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 30.0, 300.0); // cur: slight right
    let d = ahead(j, -30.0, 300.0); // alt: slight left

    let e0 = EdgeSpec {
        name: "A6",
        class: GUIDE_CLASS_PRIMARY,
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "Route B",
        class: GUIDE_CLASS_PRIMARY,
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        name: "Route C",
        class: GUIDE_CLASS_PRIMARY,
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::Fork);
    assert_eq!(steps[1].modifier, ManeuverModifier::SlightRight);
    assert_eq!(steps[1].name, "Route B");
}

/// Case 7: Motorway exit: mainline continues straight, the route takes the
/// slightly-diverging link -> OffRamp/SlightRight with signage resolved
/// from DEST_SIGNS (dest/dest_ref) and EXIT_REFS (exit_ref fallback, since
/// this DestSign's own junction_ref is absent).
#[test]
fn motorway_offramp_resolves_signage() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 1000.0);
    let b = f32pair(j);
    let c = ahead(j, 30.0, 300.0); // cur: link, slight right
    let d = ahead(j, 0.0, 1000.0); // alt: motorway continues straight

    let e0 = EdgeSpec {
        name: "A6",
        class: GUIDE_CLASS_MOTORWAY,
        ..EdgeSpec::new(0, 1, 1000.0)
    };
    let cur = EdgeSpec {
        name: "Exit 5",
        class: GUIDE_CLASS_MOTORWAY,
        link: true,
        dest: Some(("Bruxelles", "A6", "")),
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        name: "A6",
        class: GUIDE_CLASS_MOTORWAY,
        ..EdgeSpec::new(1, 3, 1000.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[(1, "12")]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::OffRamp);
    assert_eq!(steps[1].modifier, ManeuverModifier::SlightRight);
    assert_eq!(steps[1].name, "Exit 5");
    assert_eq!(steps[1].dest, "Bruxelles");
    assert_eq!(steps[1].dest_ref, "A6");
    assert_eq!(steps[1].exit_ref, "12");
}

/// Case 8: Same junction shape as the OffRamp case, but the route takes the
/// mainline -> suppressed (fork-suppression: the near-straight alternative
/// is a link; equally class-obvious if reached via rule 6).
#[test]
fn mainline_continuation_past_a_link_is_suppressed() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 1000.0);
    let b = f32pair(j);
    let c = ahead(j, 0.0, 1000.0); // cur: motorway continues straight
    let d = ahead(j, 30.0, 300.0); // alt: link diverges slight right

    let e0 = EdgeSpec {
        name: "A6",
        class: GUIDE_CLASS_MOTORWAY,
        ..EdgeSpec::new(0, 1, 1000.0)
    };
    let cur = EdgeSpec {
        name: "A6",
        class: GUIDE_CLASS_MOTORWAY,
        ..EdgeSpec::new(1, 2, 1000.0)
    };
    let alt = EdgeSpec {
        name: "Exit 5",
        class: GUIDE_CLASS_MOTORWAY,
        link: true,
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 2);
}

/// Case 9: Roundabout, route takes the 2nd available exit (a 3rd/4th further
/// around the ring are never reached, since the route leaves at the 2nd) ->
/// one Roundabout step, exit_count == 2, name of the exit road, location at
/// the entry junction.
#[test]
fn roundabout_second_exit() {
    let r0 = (49.55, 6.05);
    let r1 = (49.551, 6.051);
    let r2 = (49.552, 6.052);

    let a = behind(r0, 0.0, 500.0);
    let r0n = f32pair(r0);
    let r1n = f32pair(r1);
    let x1 = ahead(r1, 200.0, 100.0); // exit branch at r1, NOT taken
    let r2n = f32pair(r2);
    let c = ahead(r2, 45.0, 300.0); // exit road taken at r2

    // node indices: 0=a, 1=r0, 2=r1, 3=x1, 4=r2, 5=c
    let nodes = [a, r0n, r1n, x1, r2n, c];
    let e0 = EdgeSpec {
        name: "Approach Rd",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let e1 = EdgeSpec {
        roundabout: true,
        ..EdgeSpec::new(1, 2, 80.0)
    };
    let exit_branch = EdgeSpec {
        name: "Skipped Exit",
        ..EdgeSpec::new(2, 3, 100.0)
    };
    let e2 = EdgeSpec {
        roundabout: true,
        ..EdgeSpec::new(2, 4, 80.0)
    };
    let e3 = EdgeSpec {
        name: "Exit Road 2",
        ..EdgeSpec::new(4, 5, 300.0)
    };
    let (_dir, pack) = open_pack(&nodes, &[e0, e1, exit_branch, e2, e3], &[]);

    // Slots: e0=0 (node0 bucket), e1=1 (node1 bucket), exit_branch=2 and
    // e2=3 (node2 bucket, insertion order), e3=4 (node4 bucket).
    let steps = steps_for_route(&pack, &[0, 1, 3, 4]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].maneuver, ManeuverType::Depart);
    assert_eq!(steps[1].maneuver, ManeuverType::Roundabout);
    assert_eq!(steps[1].exit_count, Some(2));
    assert_eq!(steps[1].name, "Exit Road 2");
    assert_eq!(steps[1].dist_from_leg_start_m, 500.0);
    assert_eq!(steps[1].lat, r0n.0 as f64);
    assert_eq!(steps[1].lon, r0n.1 as f64);
    assert_eq!(steps[2].maneuver, ManeuverType::Arrive);
    assert_eq!(steps[2].dist_from_leg_start_m, 960.0);
}

/// Case 10: Depart/Arrive invariants over a multi-junction route: every
/// interior junction here has no alternatives (a single-path road with
/// changing names), so each one falls straight to rule 7 and emits a
/// Continue -- the point of this test is the distance bookkeeping, not the
/// classification.
#[test]
fn depart_and_arrive_bookend_every_route_with_increasing_distances() {
    let nodes = [
        (49.500, 6.000),
        (49.501, 6.000),
        (49.503, 6.000),
        (49.504, 6.000),
        (49.506, 6.000),
    ];
    let e0 = EdgeSpec {
        name: "Segment 1",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let e1 = EdgeSpec {
        name: "Segment 2",
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let e2 = EdgeSpec {
        name: "Segment 3",
        ..EdgeSpec::new(2, 3, 200.0)
    };
    let e3 = EdgeSpec {
        name: "Segment 4",
        ..EdgeSpec::new(3, 4, 400.0)
    };
    let (_dir, pack) = open_pack(&nodes, &[e0, e1, e2, e3], &[]);

    let steps = steps_for_route(&pack, &[0, 1, 2, 3]);
    assert_eq!(
        steps.first().map(|s| s.maneuver),
        Some(ManeuverType::Depart)
    );
    assert_eq!(steps.first().map(|s| s.dist_from_leg_start_m), Some(0.0));
    assert_eq!(steps.last().map(|s| s.maneuver), Some(ManeuverType::Arrive));
    assert_eq!(
        steps.last().map(|s| s.dist_from_leg_start_m),
        Some(500.0 + 300.0 + 200.0 + 400.0)
    );
    for w in steps.windows(2) {
        assert!(
            w[1].dist_from_leg_start_m > w[0].dist_from_leg_start_m,
            "distances must strictly increase: {:?} -> {:?}",
            w[0],
            w[1]
        );
    }
}

/// Case 11: A <30 m named blip between two junctions collapses away, while the
/// junction after it (also a name change) survives.
#[test]
fn short_named_blip_is_collapsed() {
    let nodes = [
        (49.500, 6.000),
        (49.501, 6.000),
        (49.5011, 6.000), // 10m-ish blip node, placed by explicit length_m below
        (49.505, 6.000),
    ];
    let e0 = EdgeSpec {
        name: "Main St",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let blip = EdgeSpec {
        name: "Blip St",
        ..EdgeSpec::new(1, 2, 10.0)
    };
    let e2 = EdgeSpec {
        name: "Main St",
        ..EdgeSpec::new(2, 3, 500.0)
    };
    let (_dir, pack) = open_pack(&nodes, &[e0, blip, e2], &[]);

    let steps = steps_for_route(&pack, &[0, 1, 2]);
    assert!(
        !steps.iter().any(|s| s.name == "Blip St"),
        "the short blip's Continue step should be collapsed: {steps:?}"
    );
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::Continue);
    assert_eq!(steps[1].name, "Main St");
    assert_eq!(steps[1].dist_from_leg_start_m, 510.0);
}

/// Case 12: A v2 pack with empty (writer-synthesized) guidance still classifies
/// gracefully (Depart/Arrive only, since every edge is unnamed); a v1 pack
/// (byte-patched exactly like `pipeline::tests::rpack_roundtrip::
/// v1_pack_back_compat`) returns no steps at all.
#[test]
fn v1_pack_and_empty_guidance_v2_pack_are_handled_gracefully() {
    let model = RegionGraphModel {
        nodes: vec![
            NodeRecord {
                lat: 49.5,
                lon: 6.0,
            },
            NodeRecord {
                lat: 49.501,
                lon: 6.0,
            },
        ],
        csr_first_edge: vec![0, 1, 1],
        edges: vec![EdgeHot {
            target: 1,
            length_m: 100.0,
            speed_kmh: 50.0,
            ascent_m: 0.0,
            descent_m: 0.0,
            road_class: 0,
            guide_flags: 0,
            _pad: [0, 0],
            ch_middle_node: CH_MIDDLE_NODE_NONE,
            geom_offset: 0,
            geom_count: 2,
        }],
        ch_order: vec![0, 1],
        geometry: vec![
            GeomVertex {
                lat: 49.5,
                lon: 6.0,
                elev_m: 0,
                _pad: 0,
            },
            GeomVertex {
                lat: 49.501,
                lon: 6.0,
                elev_m: 0,
                _pad: 0,
            },
        ],
        snap_grid: SnapGridModel {
            min_lat: 49.5,
            min_lon: 6.0,
            cell_size_deg: 1.0,
            n_rows: 1,
            n_cols: 1,
            cell_offsets: vec![0, 2],
            node_ids: vec![0, 1],
        },
        ..Default::default()
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v1compat.rpack");
    write_rpack(
        &model,
        &PackMeta {
            osm_snapshot_epoch: 0,
            region_id: 0,
            region_name: "t".to_string(),
        },
        &path,
    )
    .expect("write v1compat.rpack");

    // A v2 pack whose guidance arrays were all empty: the writer synthesizes
    // a minimal valid guidance (every edge unnamed), so classification still
    // runs without panicking, yielding just Depart/Arrive.
    let pack_v2 = Rpack::open(&path).expect("open as v2");
    assert!(pack_v2.has_guidance());
    let steps = steps_for_route(&pack_v2, &[0]);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].maneuver, ManeuverType::Depart);
    assert_eq!(steps[1].maneuver, ManeuverType::Arrive);

    // Byte-patch to a true v1 file (major/minor -> 1.1, section_count -> 8),
    // exactly like `pipeline/tests/rpack_roundtrip.rs::v1_pack_back_compat`.
    let mut bytes = std::fs::read(&path).expect("read patched file");
    bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
    bytes[6..8].copy_from_slice(&1u16.to_le_bytes());
    bytes[52..56].copy_from_slice(&8u32.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write patched file");

    let pack_v1 = Rpack::open(&path).expect("open as v1");
    assert!(!pack_v1.has_guidance());
    assert_eq!(steps_for_route(&pack_v1, &[0]), Vec::new());
}

/// Case 13: OnRamp: a residential road's link diverges toward a motorway (prev
/// class is neither motorway nor trunk -> OnRamp, not OffRamp).
#[test]
fn onramp_from_residential_road() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 300.0);
    let b = f32pair(j);
    let c = ahead(j, 30.0, 150.0); // cur: link toward motorway, slight right
    let d = ahead(j, 0.0, 300.0); // alt: residential continues straight

    let e0 = EdgeSpec {
        name: "Local Rd",
        class: GUIDE_CLASS_RESIDENTIAL,
        ..EdgeSpec::new(0, 1, 300.0)
    };
    let cur = EdgeSpec {
        name: "Ramp to A6",
        class: GUIDE_CLASS_MOTORWAY,
        link: true,
        ..EdgeSpec::new(1, 2, 150.0)
    };
    let alt = EdgeSpec {
        name: "Local Rd",
        class: GUIDE_CLASS_RESIDENTIAL,
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::OnRamp);
    assert_eq!(steps[1].modifier, ManeuverModifier::SlightRight);
    assert_eq!(steps[1].name, "Ramp to A6");
}

/// Case 14: UTurn modifier: deviation > 170 degrees with an alternative present.
#[test]
fn uturn_modifier_emitted_past_170_degrees() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 400.0);
    let b = f32pair(j);
    let c = ahead(j, 175.0, 300.0); // cur: near U-turn
    let d = ahead(j, 10.0, 300.0); // alt: near-straight (keeps EndOfRoad from firing)

    let e0 = EdgeSpec {
        name: "Rue A",
        ..EdgeSpec::new(0, 1, 400.0)
    };
    let cur = EdgeSpec {
        name: "Rue A Return",
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        name: "Rue C",
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::Turn);
    assert_eq!(steps[1].modifier, ManeuverModifier::UTurn);
    assert_eq!(steps[1].name, "Rue A Return");
}

/// Case 15a: Near-straight fork where `cur` continues under prev's same
/// non-empty name (dev <= 20) -> the Fork is suppressed as same-name
/// straight (OSRM obviousness), and no Continue fires either since the
/// names match.
#[test]
fn same_name_straight_fork_is_suppressed() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 15.0, 300.0); // cur: barely off straight, same name
    let d = ahead(j, -30.0, 300.0); // alt: slight left branch

    let e0 = EdgeSpec {
        name: "Mechelsesteenweg",
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        name: "Mechelsesteenweg",
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        name: "Zijstraat",
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(
        steps.len(),
        2,
        "same-name straight fork must be silent: {steps:?}"
    );
    assert_eq!(steps[0].maneuver, ManeuverType::Depart);
    assert_eq!(steps[1].maneuver, ManeuverType::Arrive);
}

/// Case 15b: The motorway-split protection for 15a's suppression: identical
/// geometry, but every edge UNNAMED sharing one ref (a mainline splitting
/// into two unnamed carriageways, like the A2 Amsterdam keep-left) -> the
/// Fork must still be emitted. This is why the suppression tests NAME
/// equality only, never `similar()`'s empty-names-equal-refs fallback.
#[test]
fn unnamed_same_ref_fork_still_emits() {
    let j = (49.55, 6.05);
    let a = behind(j, 0.0, 500.0);
    let b = f32pair(j);
    let c = ahead(j, 15.0, 300.0); // cur: barely off straight
    let d = ahead(j, -30.0, 300.0); // alt: the other branch

    let e0 = EdgeSpec {
        road_ref: "A2",
        class: GUIDE_CLASS_MOTORWAY,
        ..EdgeSpec::new(0, 1, 500.0)
    };
    let cur = EdgeSpec {
        road_ref: "A2",
        class: GUIDE_CLASS_MOTORWAY,
        ..EdgeSpec::new(1, 2, 300.0)
    };
    let alt = EdgeSpec {
        road_ref: "A2",
        class: GUIDE_CLASS_MOTORWAY,
        ..EdgeSpec::new(1, 3, 300.0)
    };
    let nodes = [a, b, c, d];
    let (_dir, pack) = open_pack(&nodes, &[e0, cur, alt], &[]);

    let steps = steps_for_route(&pack, &[0, 1]);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[1].maneuver, ManeuverType::Fork);
    assert_eq!(steps[1].modifier, ManeuverModifier::SlightRight);
    assert_eq!(steps[1].road_ref, "A2");
}
