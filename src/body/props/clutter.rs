//! The clutter rules (pure): which old things she tidies up, and where she puts something down.

use rand::RngExt;

use super::store::Placed;

/// Oldest first: everything older than `min_age_s`, plus - whatever their age - the oldest beyond the
/// `max_placed` cap. `keep` (the chair she sits on) never.
pub fn tidy_candidates(
    placed: &[Placed],
    now: f64,
    min_age_s: f64,
    max_placed: usize,
    keep: &[String],
) -> Vec<Placed> {
    let mut items: Vec<Placed> = placed
        .iter()
        .filter(|p| !keep.contains(&p.id))
        .cloned()
        .collect();
    items.sort_by(|a, b| a.placed_at.total_cmp(&b.placed_at));
    let over = (items.len() + keep.len()).saturating_sub(max_placed);
    items
        .iter()
        .enumerate()
        .filter(|(i, p)| *i < over || now - p.placed_at >= min_age_s)
        .map(|(_, p)| p.clone())
        .collect()
}

pub struct Spot<'a> {
    pub near_x: f64,
    pub roam: (f64, f64),
    pub occupied: &'a [f64],
    pub focus: Option<(f64, f64)>,
    pub spacing: f64,
}

/// Where to put something down: weighted over the floor within 1.6 m of her (anywhere if that's all taken),
/// nearer is likelier, never on top of another item, and - a weight, not a rule - away from the focused window.
pub fn pick_spot(s: &Spot) -> f64 {
    pick_within(s, 1.6)
}

fn pick_within(s: &Spot, reach: f64) -> f64 {
    const STEPS: usize = 41;
    let (lo, hi) = s.roam;
    let xs: Vec<f64> = (0..STEPS)
        .map(|i| lo + (hi - lo) * i as f64 / (STEPS - 1) as f64)
        .collect();
    let weights: Vec<f64> = xs
        .iter()
        .map(|&x| {
            if s.occupied.iter().any(|o| (x - o).abs() < s.spacing) {
                return 0.0;
            }
            let d = (x - s.near_x).abs();
            if d > reach {
                return 0.0;
            }
            let mut w = 1.0 / (1.0 + (d / 0.5).powi(2));
            if let Some((fx, fhw)) = s.focus
                && (x - fx).abs() < fhw
            {
                w *= 0.2;
            }
            w
        })
        .collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        if reach < hi - lo {
            return pick_within(s, hi - lo + 1.0); // nothing free near her: anywhere on the floor
        }
        return s.near_x.clamp(lo, hi);
    }
    let mut r = rand::rng().random_range(0.0..total);
    for (x, w) in xs.iter().zip(&weights) {
        if r < *w {
            return *x;
        }
        r -= w;
    }
    *xs.last().unwrap_or(&s.near_x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, at: f64) -> Placed {
        Placed {
            id: id.into(),
            item: "x".into(),
            x: 0.0,
            z: 0.0,
            placed_at: at,
        }
    }

    #[test]
    fn tidy_takes_the_old_and_the_overflow_never_the_chair() {
        let items = [p("a", 0.0), p("b", 100.0), p("chair", 50.0), p("c", 9000.0)];
        let t = tidy_candidates(&items, 10_000.0, 3600.0, 3, &["chair".into()]);
        let ids: Vec<&str> = t.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn spots_avoid_other_items() {
        let occupied = [0.0, 0.5, -0.5];
        for _ in 0..100 {
            let x = pick_spot(&Spot {
                near_x: 0.0,
                roam: (-3.0, 3.0),
                occupied: &occupied,
                focus: None,
                spacing: 0.45,
            });
            assert!(occupied.iter().all(|o| (x - o).abs() >= 0.45), "{x}");
        }
    }
}
