//! Maneuver generation (wayfinder #66): a pure route-time pass that derives
//! a typed step list from one Leg's already-unpacked `route_edges`, using
//! the format 2.0 guidance sections (wayfinder #65). Steps carry structured
//! data only -- no baked English sentences (that's app-side, #67).
//!
//! Modeled on GraphHopper's `InstructionsFromEdges` (query-time
//! classification over unpacked path edges), tuned with OSRM's angle
//! constants; see `docs/research/turn-by-turn.md` for the survey this
//! implements.

mod geometry;

use geometry::{delta_from, entry_bearing, from_node};
use packs::{
    EdgeHot, Rpack, CH_MIDDLE_NODE_NONE, GUIDE_CLASS_LIVING_STREET, GUIDE_CLASS_MOTORWAY,
    GUIDE_CLASS_PRIMARY, GUIDE_CLASS_RESIDENTIAL, GUIDE_CLASS_SECONDARY, GUIDE_CLASS_TERTIARY,
    GUIDE_CLASS_TRUNK, GUIDE_CLASS_UNCLASSIFIED, GUIDE_NONE,
};

/// OSRM-derived deviation-from-straight thresholds, in degrees (see
/// `docs/research/turn-by-turn.md` §1.2-1.3).
const DEV_STRAIGHT: f64 = 20.0;
const DEV_SLIGHT: f64 = 40.0;
const DEV_SHARP: f64 = 120.0;
const DEV_UTURN: f64 = 170.0;
const FUZZ: f64 = 25.0;
const ANGLE_RATIO: f64 = 1.4;
const SLOWER_FACTOR: f32 = 2.0;
/// Short-Continue collapse distance (reduced form of OSRM's 30 m collapse).
const MIN_SEGMENT_M: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManeuverType {
    Depart,
    Arrive,
    Turn,
    Continue,
    OffRamp,
    OnRamp,
    Fork,
    EndOfRoad,
    Roundabout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManeuverModifier {
    Straight,
    SlightLeft,
    SlightRight,
    Left,
    Right,
    SharpLeft,
    SharpRight,
    UTurn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub maneuver: ManeuverType,
    pub modifier: ManeuverModifier,
    /// Roundabout only: which exit to take (>= 1).
    pub exit_count: Option<u32>,
    /// Road AFTER the maneuver (resolved strings; empty when absent).
    pub name: String,
    pub road_ref: String,
    /// Signage (OffRamp/OnRamp/Fork, from DEST_SIGNS / EXIT_REFS; empty when absent).
    pub dest: String,
    pub dest_ref: String,
    pub exit_ref: String,
    /// Junction location (the node where the maneuver happens).
    pub lat: f64,
    pub lon: f64,
    /// Sum of `EdgeHot.length_m` of the leg's edges before this maneuver.
    pub dist_from_leg_start_m: f64,
}

/// Steps for one Leg's unpacked original edges, in travel order. Empty when
/// `route_edges` is empty or `!pack.has_guidance()` (a v1 pack means
/// route-following only, ADR 0012's base behaviour).
pub fn steps_for_route(pack: &Rpack, route_edges: &[u32]) -> Vec<Step> {
    if !pack.has_guidance() || route_edges.is_empty() {
        return Vec::new();
    }

    let edges = pack.edges();
    let n = route_edges.len();

    let first_slot = route_edges[0] as usize;
    let first_edge = &edges[first_slot];
    let node0 = pack.nodes()[from_node(pack, first_slot) as usize];
    let (name0, ref0) = name_ref(pack, first_slot);

    let mut steps = vec![Step {
        maneuver: ManeuverType::Depart,
        modifier: ManeuverModifier::Straight,
        exit_count: None,
        name: name0,
        road_ref: ref0,
        dest: String::new(),
        dest_ref: String::new(),
        exit_ref: String::new(),
        lat: node0.lat as f64,
        lon: node0.lon as f64,
        dist_from_leg_start_m: 0.0,
    }];

    // A route that starts mid-roundabout is treated as already circulating,
    // with the entry remembered at the route start.
    let mut roundabout: Option<RoundaboutState> =
        first_edge.guide_is_roundabout().then_some(RoundaboutState {
            entry_lat: node0.lat as f64,
            entry_lon: node0.lon as f64,
            entry_dist: 0.0,
            exit_count: 0,
        });

    let mut cum_m: f64 = 0.0;
    for i in 1..n {
        let prev_slot = route_edges[i - 1] as usize;
        let cur_slot = route_edges[i] as usize;
        let prev_edge = &edges[prev_slot];
        let cur_edge = &edges[cur_slot];
        cum_m += prev_edge.length_m as f64;
        let node_id = prev_edge.target;
        let node = pack.nodes()[node_id as usize];

        // 1. Roundabout state machine -- takes precedence over everything.
        if !prev_edge.guide_is_roundabout() && cur_edge.guide_is_roundabout() {
            roundabout = Some(RoundaboutState {
                entry_lat: node.lat as f64,
                entry_lon: node.lon as f64,
                entry_dist: cum_m,
                exit_count: 0,
            });
            continue;
        }
        if prev_edge.guide_is_roundabout() {
            let state = roundabout
                .as_mut()
                .expect("circulating implies an open roundabout state");
            let has_exit_option = originals_at(pack, node_id)
                .iter()
                .any(|&(_, e)| !e.guide_is_roundabout());
            if has_exit_option {
                state.exit_count += 1;
            }
            if cur_edge.guide_is_roundabout() {
                continue;
            }
            let (name, road_ref) = name_ref(pack, cur_slot);
            let exit_count = state.exit_count.max(1);
            let (lat, lon, dist) = (state.entry_lat, state.entry_lon, state.entry_dist);
            roundabout = None;
            steps.push(Step {
                maneuver: ManeuverType::Roundabout,
                modifier: ManeuverModifier::Straight,
                exit_count: Some(exit_count),
                name,
                road_ref,
                dest: String::new(),
                dest_ref: String::new(),
                exit_ref: String::new(),
                lat,
                lon,
                dist_from_leg_start_m: dist,
            });
            continue;
        }

        // 2. Alternatives: outgoing original edges at this node, excluding
        // `cur` (by slot) and the immediate U-turn back-edge.
        let from_prev = from_node(pack, prev_slot);
        let alternatives: Vec<(usize, &EdgeHot)> = originals_at(pack, node_id)
            .into_iter()
            .filter(|&(slot, e)| slot != cur_slot && e.target != from_prev)
            .collect();

        if alternatives.is_empty() {
            emit_rename_fallback(pack, &mut steps, prev_slot, cur_slot, node, cum_m);
            continue;
        }

        let entry = entry_bearing(pack, prev_slot, prev_edge);
        let delta = delta_from(pack, entry, cur_slot, cur_edge);
        let dev = delta.abs();

        // 3. Ramp entry.
        if cur_edge.guide_is_link() && !prev_edge.guide_is_link() {
            let modifier = modifier_from_delta(delta);
            let maneuver = if matches!(
                prev_edge.guide_class(),
                GUIDE_CLASS_MOTORWAY | GUIDE_CLASS_TRUNK
            ) {
                ManeuverType::OffRamp
            } else {
                ManeuverType::OnRamp
            };
            let (name, road_ref) = name_ref(pack, cur_slot);
            let (dest, dest_ref, exit_ref) = dest_signage(pack, cur_slot, node_id);
            steps.push(Step {
                maneuver,
                modifier,
                exit_count: None,
                name,
                road_ref,
                dest,
                dest_ref,
                exit_ref,
                lat: node.lat as f64,
                lon: node.lon as f64,
                dist_from_leg_start_m: cum_m,
            });
            continue;
        }

        // 4. Fork: `cur` and some alternative are both near-straight. Two
        // obviousness suppressions apply, both falling through to rule 7:
        // (a) obvious main road: `cur` keeps prev's class + link bit and
        //     every near-straight alternative is a link;
        // (b) same-name straight (OSRM: a continuation "perfectly straight
        //     with the same name" is obvious): dev <= DEV_STRAIGHT and
        //     `cur`'s non-empty NAME equals prev's. Name equality only --
        //     deliberately not [`similar`], whose empty-names-equal-refs
        //     fallback would also swallow genuine motorway splits where both
        //     branches are unnamed and share the mainline ref.
        let near_straight_alts: Vec<&(usize, &EdgeHot)> = alternatives
            .iter()
            .filter(|&&(slot, e)| delta_from(pack, entry, slot, e).abs() <= DEV_SLIGHT)
            .collect();
        if dev <= DEV_SLIGHT && !near_straight_alts.is_empty() {
            let keeps_class = cur_edge.guide_class() == prev_edge.guide_class()
                && cur_edge.guide_is_link() == prev_edge.guide_is_link();
            let all_alts_link = near_straight_alts.iter().all(|&&(_, e)| e.guide_is_link());
            let (name_prev, _) = name_ref(pack, prev_slot);
            let (name_cur, _) = name_ref(pack, cur_slot);
            let same_name_straight =
                dev <= DEV_STRAIGHT && !name_cur.is_empty() && name_cur == name_prev;
            if !(keeps_class && all_alts_link) && !same_name_straight {
                let modifier = if delta < 0.0 {
                    ManeuverModifier::SlightLeft
                } else {
                    ManeuverModifier::SlightRight
                };
                let (name, road_ref) = name_ref(pack, cur_slot);
                let (dest, dest_ref, exit_ref) = dest_signage(pack, cur_slot, node_id);
                steps.push(Step {
                    maneuver: ManeuverType::Fork,
                    modifier,
                    exit_count: None,
                    name,
                    road_ref,
                    dest,
                    dest_ref,
                    exit_ref,
                    lat: node.lat as f64,
                    lon: node.lon as f64,
                    dist_from_leg_start_m: cum_m,
                });
                continue;
            }
            // Suppressed (obvious main road, or same-name straight): fall
            // through to rule 7.
            emit_rename_fallback(pack, &mut steps, prev_slot, cur_slot, node, cum_m);
            continue;
        }

        // 5. End of road (T-junction).
        if dev > DEV_SLIGHT {
            let no_near_straight = originals_at(pack, node_id)
                .into_iter()
                .filter(|&(_, e)| e.target != from_prev)
                .all(|(slot, e)| delta_from(pack, entry, slot, e).abs() > DEV_SLIGHT);
            if no_near_straight {
                let modifier = modifier_from_delta(delta);
                let (name, road_ref) = name_ref(pack, cur_slot);
                steps.push(Step {
                    maneuver: ManeuverType::EndOfRoad,
                    modifier,
                    exit_count: None,
                    name,
                    road_ref,
                    dest: String::new(),
                    dest_ref: String::new(),
                    exit_ref: String::new(),
                    lat: node.lat as f64,
                    lon: node.lon as f64,
                    dist_from_leg_start_m: cum_m,
                });
                continue;
            }
        }

        // 6. Turn.
        let modifier = modifier_from_delta(delta);
        if dev <= DEV_SLIGHT {
            let angle_obvious = alternatives.iter().all(|&(slot, e)| {
                delta_from(pack, entry, slot, e).abs() > (ANGLE_RATIO * dev).max(dev + FUZZ)
            });
            let class_obvious = dev <= DEV_STRAIGHT
                && cur_edge.guide_class() == prev_edge.guide_class()
                && cur_edge.guide_is_link() == prev_edge.guide_is_link()
                && alternatives
                    .iter()
                    .all(|&(_, e)| class_priority(e) < class_priority(cur_edge));
            let similar_prev_cur = similar(pack, prev_slot, cur_slot);
            let slower_alternatives = similar_prev_cur
                && alternatives
                    .iter()
                    .all(|&(_, e)| e.speed_kmh * SLOWER_FACTOR <= cur_edge.speed_kmh);
            let plain_straight = dev <= DEV_STRAIGHT && similar_prev_cur;
            if angle_obvious || class_obvious || slower_alternatives || plain_straight {
                emit_rename_fallback(pack, &mut steps, prev_slot, cur_slot, node, cum_m);
                continue;
            }
        }
        let (name, road_ref) = name_ref(pack, cur_slot);
        steps.push(Step {
            maneuver: ManeuverType::Turn,
            modifier,
            exit_count: None,
            name,
            road_ref,
            dest: String::new(),
            dest_ref: String::new(),
            exit_ref: String::new(),
            lat: node.lat as f64,
            lon: node.lon as f64,
            dist_from_leg_start_m: cum_m,
        });
    }

    let last_slot = route_edges[n - 1] as usize;
    let last_edge = &edges[last_slot];
    cum_m += last_edge.length_m as f64;
    let dest_node = pack.nodes()[last_edge.target as usize];
    steps.push(Step {
        maneuver: ManeuverType::Arrive,
        modifier: ManeuverModifier::Straight,
        exit_count: None,
        name: String::new(),
        road_ref: String::new(),
        dest: String::new(),
        dest_ref: String::new(),
        exit_ref: String::new(),
        lat: dest_node.lat as f64,
        lon: dest_node.lon as f64,
        dist_from_leg_start_m: cum_m,
    });

    collapse_short_continues(&mut steps);
    steps
}

struct RoundaboutState {
    entry_lat: f64,
    entry_lon: f64,
    entry_dist: f64,
    exit_count: u32,
}

/// Original (non-shortcut) edges at node `n`, as `(slot, &EdgeHot)` pairs.
/// Empty if `n` is out of range.
fn originals_at(pack: &Rpack, n: u32) -> Vec<(usize, &EdgeHot)> {
    let Some(range) = pack.edge_range(n) else {
        return Vec::new();
    };
    let edges = pack.edges();
    range
        .filter(|&slot| edges[slot].ch_middle_node == CH_MIDDLE_NODE_NONE)
        .map(|slot| (slot, &edges[slot]))
        .collect()
}

/// Resolves a guidance string id to its text, or `""` for id 0, a malformed
/// id, or a v1 pack -- never panics.
fn resolve_string(pack: &Rpack, id: u32) -> String {
    pack.string(id).unwrap_or("").to_string()
}

/// `(name, road_ref)` for the edge at `edge_slot`, via `edge_guide()` ->
/// `edge_attrs()` -> `string()`. `GUIDE_NONE` or any malformed index
/// resolves to empty strings rather than panicking.
fn name_ref(pack: &Rpack, edge_slot: usize) -> (String, String) {
    let guide = pack
        .edge_guide()
        .get(edge_slot)
        .copied()
        .unwrap_or(GUIDE_NONE);
    if guide == GUIDE_NONE {
        return (String::new(), String::new());
    }
    match pack.edge_attrs().get(guide as usize) {
        Some(attr) => (
            resolve_string(pack, attr.name_id),
            resolve_string(pack, attr.ref_id),
        ),
        None => (String::new(), String::new()),
    }
}

/// Destination signage for a ramp/fork edge: `(dest, dest_ref, exit_ref)`.
/// `exit_ref` prefers the sign's `junction_ref` and falls back to the
/// junction node's own exit ref.
fn dest_signage(pack: &Rpack, edge_slot: usize, node_id: u32) -> (String, String, String) {
    let node_exit_ref = || {
        pack.exit_ref_for_node(node_id)
            .map(|id| resolve_string(pack, id))
            .unwrap_or_default()
    };
    match pack.dest_sign_for_edge(edge_slot as u32) {
        Some(sign) => {
            let dest = resolve_string(pack, sign.dest_id);
            let dest_ref = resolve_string(pack, sign.dest_ref_id);
            let junction_ref = resolve_string(pack, sign.junction_ref_id);
            let exit_ref = if !junction_ref.is_empty() {
                junction_ref
            } else {
                node_exit_ref()
            };
            (dest, dest_ref, exit_ref)
        }
        None => (String::new(), String::new(), node_exit_ref()),
    }
}

/// Whether edges `a` and `b` count as "the same road" for continuation
/// suppression: both attr name strings equal and non-empty, or both names
/// empty and refs equal (including both-empty refs). GraphHopper treats two
/// empty names as never similar; this deliberately relaxes that so an
/// unnamed chain (matching ref, or fully unattributed) doesn't spam steps.
fn similar(pack: &Rpack, a_slot: usize, b_slot: usize) -> bool {
    let (name_a, ref_a) = name_ref(pack, a_slot);
    let (name_b, ref_b) = name_ref(pack, b_slot);
    (!name_a.is_empty() && name_a == name_b)
        || (name_a.is_empty() && name_b.is_empty() && ref_a == ref_b)
}

/// Class priority table (`docs/research/turn-by-turn.md` §1.3), highest
/// first; `-1.5` when the edge is a `_link`.
fn class_priority(edge: &EdgeHot) -> f64 {
    let base = match edge.guide_class() {
        GUIDE_CLASS_MOTORWAY => 10.0,
        GUIDE_CLASS_TRUNK => 9.0,
        GUIDE_CLASS_PRIMARY => 8.0,
        GUIDE_CLASS_SECONDARY => 7.0,
        GUIDE_CLASS_TERTIARY => 6.0,
        GUIDE_CLASS_UNCLASSIFIED => 5.0,
        GUIDE_CLASS_RESIDENTIAL => 4.0,
        GUIDE_CLASS_LIVING_STREET => 3.0,
        _ => 2.0,
    };
    if edge.guide_is_link() {
        base - 1.5
    } else {
        base
    }
}

/// Modifier from a signed turn delta: OSRM-derived deviation-from-straight
/// bands (`docs/research/turn-by-turn.md` §1.2).
fn modifier_from_delta(delta: f64) -> ManeuverModifier {
    let dev = delta.abs();
    let left = delta < 0.0;
    if dev <= DEV_STRAIGHT {
        ManeuverModifier::Straight
    } else if dev <= DEV_SLIGHT {
        if left {
            ManeuverModifier::SlightLeft
        } else {
            ManeuverModifier::SlightRight
        }
    } else if dev <= DEV_SHARP {
        if left {
            ManeuverModifier::Left
        } else {
            ManeuverModifier::Right
        }
    } else if dev <= DEV_UTURN {
        if left {
            ManeuverModifier::SharpLeft
        } else {
            ManeuverModifier::SharpRight
        }
    } else {
        ManeuverModifier::UTurn
    }
}

/// Rule 7: emits a `Continue` step ("new name") only if `cur` isn't
/// [`similar`] to `prev` and actually has a name -- otherwise nothing is
/// emitted at this junction at all.
fn emit_rename_fallback(
    pack: &Rpack,
    steps: &mut Vec<Step>,
    prev_slot: usize,
    cur_slot: usize,
    node: packs::NodeRecord,
    cum_m: f64,
) {
    if similar(pack, prev_slot, cur_slot) {
        return;
    }
    let (name, road_ref) = name_ref(pack, cur_slot);
    if name.is_empty() {
        return;
    }
    steps.push(Step {
        maneuver: ManeuverType::Continue,
        modifier: ManeuverModifier::Straight,
        exit_count: None,
        name,
        road_ref,
        dest: String::new(),
        dest_ref: String::new(),
        exit_ref: String::new(),
        lat: node.lat as f64,
        lon: node.lon as f64,
        dist_from_leg_start_m: cum_m,
    });
}

/// Reduced form of OSRM's 30 m collapse pass: drops any `Continue` step
/// whose distance to the next step (route end included, since `Arrive` is
/// always last) is under [`MIN_SEGMENT_M`] -- a named blip between two real
/// junctions. Turn/ramp/fork steps are never removed here (staggered
/// junctions are legitimate).
fn collapse_short_continues(steps: &mut Vec<Step>) {
    let mut i = 0;
    while i < steps.len() {
        if steps[i].maneuver == ManeuverType::Continue {
            let next_dist = steps[i + 1..]
                .first()
                .map(|s| s.dist_from_leg_start_m)
                .unwrap_or(steps[i].dist_from_leg_start_m);
            if next_dist - steps[i].dist_from_leg_start_m < MIN_SEGMENT_M {
                steps.remove(i);
                continue;
            }
        }
        i += 1;
    }
}
