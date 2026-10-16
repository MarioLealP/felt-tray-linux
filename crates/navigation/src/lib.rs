//! Pure platform-graph pathfinding - no physics, no I/O.
//!
//! Given the world model's platforms (world.py's `Platform`, desktop px, +y
//! down), her current platform + x, and a target platform + x, [`plan_route`]
//! returns a list of [`Step`]s: [`Walk`] (walk to an x on the platform she's
//! currently on), [`HopDown`] (step off, fall to a strictly lower platform
//! whose x-range contains the landing x) and [`Climb`] (root-motion-assisted
//! step up onto a platform at most 2 m higher, whose x-range overlaps hers).
//!
//! Anything bigger than a 2 m climb, or two platforms with no overlapping x at
//! all, isn't a direct edge - the BFS then has to route via a floor (every
//! floor is mutually walk-connected: monitors sit side by side and WP-22's walk
//! already spans them, so crossing floors costs nothing here - the real
//! per-monitor y difference is what the WP-23 slice-2 support glue handles).
//!
//! `ppm` (pixels per world metre, from the overlay's own geometry report - see
//! geometry.py/loops.py's `_her_height_px`) is the only unit conversion this
//! module needs: a climb's rise in px, divided by ppm, is the rise in metres,
//! which picks `climb/1m` vs `climb/2m` (the smallest clip that covers it) or
//! rules the climb out entirely (> 2 m -> no edge, route around instead).

use std::collections::{HashMap, HashSet, VecDeque};

/// One stand-on-able ledge, desktop px. `id` is stable across ticks for the
/// same window/monitor/seam so main.gd (a later slice) can tell "she's still
/// on the same platform" from "the platform moved".
///
/// Minimal stand-in for `felt_animation.world.Platform`.
#[derive(Clone, Debug, PartialEq)]
pub struct Platform {
    pub id: String,
    pub hwnd: Option<i64>,
    pub x0: f64,
    pub x1: f64,
    pub y: f64,
    pub kind: PlatformKind,
}

/// What a [`Platform`] is cut out of (world.py's `kind`: "floor" | "window" |
/// "seam").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformKind {
    Floor,
    Window,
    Seam,
}

/// Walk to an x on the platform she's currently on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Walk {
    pub x_px: f64,
}

/// Step off, fall to a strictly lower platform whose x-range contains the
/// landing x.
#[derive(Clone, Debug, PartialEq)]
pub struct HopDown {
    pub to_platform: String,
    pub x_px: f64,
}

/// Root-motion-assisted step up onto a platform at most 2 m higher, whose
/// x-range overlaps hers. `meters` is the clip pick: 1 or 2.
#[derive(Clone, Debug, PartialEq)]
pub struct Climb {
    pub to_platform: String,
    pub x_px: f64,
    pub meters: u32,
}

/// One leg of a route plan.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Walk(Walk),
    HopDown(HopDown),
    Climb(Climb),
}

/// A climb clip is picked by whichever of these its rise fits under first,
/// smallest first ("use the smallest clip that covers the rise" - the WP's
/// own wording). A small tolerance covers a real platform pair whose rise is
/// a hair over 1.0/2.0 "meters" due to px/ppm rounding.
const CLIMB_TOLERANCE: f64 = 1.05;
const CLIMB_METERS: [u32; 2] = [1, 2];
/// Two platforms within this many px of each other's y count as "the same
/// height" - neither a hop nor a climb, just walking (floor-to-floor is
/// handled separately below; this guards window/seam edges from a
/// hop-to-self on a near-zero height difference).
const SAME_HEIGHT_TOLERANCE_PX: f64 = 1.0;

/// A platform-graph edge. `kind` is "walk" | "hop" | "climb".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeKind {
    Walk,
    Hop,
    Climb,
}

#[derive(Clone, Debug)]
struct Edge {
    kind: EdgeKind,
    to_id: String,
    x_px: f64,   // meaningless for "walk" (transparent - any x reachable)
    meters: u32, // meaningless for "walk"/"hop"
}

fn x_overlap(a: &Platform, b: &Platform) -> Option<(f64, f64)> {
    let (lo, hi) = (a.x0.max(b.x0), a.x1.min(b.x1));
    (hi > lo).then_some((lo, hi))
}

/// The smallest clip (1 or 2 m) whose reach covers `rise_px`, or None if
/// it's more than a 2 m step (or `ppm` is unusable).
fn climb_meters_for_rise(rise_px: f64, ppm: f64) -> Option<u32> {
    if ppm <= 0.0 {
        return None;
    }
    let rise_m = rise_px / ppm;
    CLIMB_METERS
        .into_iter()
        .find(|&meters| rise_m <= f64::from(meters) * CLIMB_TOLERANCE)
}

fn build_edges(platforms: &[Platform], ppm: f64) -> HashMap<String, Vec<Edge>> {
    let mut edges: HashMap<String, Vec<Edge>> = HashMap::new();
    for p in platforms {
        edges.entry(p.id.clone()).or_default();
    }

    let floors: Vec<&Platform> = platforms
        .iter()
        .filter(|p| p.kind == PlatformKind::Floor)
        .collect();
    for a in &floors {
        for b in &floors {
            if a.id != b.id {
                edges.entry(a.id.clone()).or_default().push(Edge {
                    kind: EdgeKind::Walk,
                    to_id: b.id.clone(),
                    x_px: 0.0,
                    meters: 0,
                });
            }
        }
    }

    for a in platforms {
        for b in platforms {
            if a.id == b.id {
                continue;
            }
            let Some((lo, hi)) = x_overlap(a, b) else {
                continue;
            };
            let mid = (lo + hi) / 2.0;
            let drop = b.y - a.y; // positive: b is lower (bigger y = lower on screen)
            if drop > SAME_HEIGHT_TOLERANCE_PX {
                edges.entry(a.id.clone()).or_default().push(Edge {
                    kind: EdgeKind::Hop,
                    to_id: b.id.clone(),
                    x_px: mid,
                    meters: 0,
                });
            } else if drop < -SAME_HEIGHT_TOLERANCE_PX
                && let Some(meters) = climb_meters_for_rise(-drop, ppm)
            {
                edges.entry(a.id.clone()).or_default().push(Edge {
                    kind: EdgeKind::Climb,
                    to_id: b.id.clone(),
                    x_px: mid,
                    meters,
                });
            }
        }
    }

    edges
}

fn bfs(edges: &HashMap<String, Vec<Edge>>, start: &str, goal: &str) -> Option<Vec<Edge>> {
    if !edges.contains_key(start) || !edges.contains_key(goal) {
        return None;
    }
    if start == goal {
        return Some(Vec::new());
    }
    let mut came_from: HashMap<String, (String, Edge)> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::from([start.to_string()]);
    let mut queue: VecDeque<String> = VecDeque::from([start.to_string()]);
    while let Some(node) = queue.pop_front() {
        for edge in &edges[&node] {
            if visited.contains(&edge.to_id) {
                continue;
            }
            visited.insert(edge.to_id.clone());
            came_from.insert(edge.to_id.clone(), (node.clone(), edge.clone()));
            if edge.to_id == goal {
                let mut path = Vec::new();
                let mut cur = goal.to_string();
                while cur != start {
                    let (prev, e) = came_from[&cur].clone();
                    path.push(e);
                    cur = prev;
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(edge.to_id.clone());
        }
    }
    None
}

/// None if `target_platform_id` isn't in `platforms` at all, or there's no
/// path (a climb too tall with nothing to walk around it, an isolated
/// platform). Otherwise a (possibly empty other than the final Walk) plan:
/// Walk to the takeoff x on her own platform, HopDown/Climb across, repeat,
/// then a final Walk to `target_x_px` on the target platform. Floor-to-floor
/// legs of the path (any number of them) are transparent - she just keeps
/// walking; see the module docstring for why that's safe to assume here.
///
/// `current_x_px` mirrors the Python parameter (accepted for call-site
/// parity, unused there too - the takeoff x always comes from the edge).
pub fn plan_route(
    platforms: &[Platform],
    ppm: f64,
    current_platform_id: &str,
    _current_x_px: f64,
    target_platform_id: &str,
    target_x_px: f64,
) -> Option<Vec<Step>> {
    let by_id: HashMap<&str, &Platform> = platforms.iter().map(|p| (p.id.as_str(), p)).collect();
    if !by_id.contains_key(current_platform_id) || !by_id.contains_key(target_platform_id) {
        return None;
    }
    if current_platform_id == target_platform_id {
        return Some(vec![Step::Walk(Walk { x_px: target_x_px })]);
    }

    let edges = build_edges(platforms, ppm);
    let path = bfs(&edges, current_platform_id, target_platform_id)?;

    let mut steps: Vec<Step> = Vec::new();
    for edge in path {
        match edge.kind {
            EdgeKind::Walk => {} // transparent - see docstring
            EdgeKind::Hop => {
                steps.push(Step::Walk(Walk { x_px: edge.x_px }));
                steps.push(Step::HopDown(HopDown {
                    to_platform: edge.to_id,
                    x_px: edge.x_px,
                }));
            }
            EdgeKind::Climb => {
                steps.push(Step::Walk(Walk { x_px: edge.x_px }));
                steps.push(Step::Climb(Climb {
                    to_platform: edge.to_id,
                    x_px: edge.x_px,
                    meters: edge.meters,
                }));
            }
        }
    }
    steps.push(Step::Walk(Walk { x_px: target_x_px }));
    Some(steps)
}

pub const LEDGE_MIN_WIDTH_PX: f64 = 150.0;
pub const LEDGE_EDGE_MARGIN_PX: f64 = 60.0;

/// Mario's "force her onto a ledge" (2026-09-27): every non-floor platform
/// wide enough to stand on, nearest to `point_px` first (the cursor - aim at
/// a window, press the key), each paired with the landing x: the point's x
/// clamped inside the ledge, away from its ends. Her own platform is skipped.
/// The caller tries them in order until one routes.
pub fn ledges_near(
    platforms: &[Platform],
    point_px: (f64, f64),
    current_platform_id: Option<&str>,
    min_width_px: f64,
) -> Vec<(Platform, f64)> {
    let (px, py) = point_px;
    let mut out: Vec<(Platform, f64, f64)> = Vec::new();
    for p in platforms {
        if p.kind == PlatformKind::Floor
            || Some(p.id.as_str()) == current_platform_id
            || (p.x1 - p.x0) < min_width_px
        {
            continue;
        }
        let margin = LEDGE_EDGE_MARGIN_PX.min((p.x1 - p.x0) / 2.0);
        let x = (px.max(p.x0 + margin)).min(p.x1 - margin);
        out.push((p.clone(), x, (x - px).powi(2) + (p.y - py).powi(2)));
    }
    out.sort_by(|a, b| a.2.total_cmp(&b.2));
    out.into_iter().map(|(p, x, _)| (p, x)).collect()
}
