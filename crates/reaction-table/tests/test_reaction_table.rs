//! reaction_table.py (WP-15): mood-weighted, no-immediate-repeat picking
//! from a small data-driven table. Pure function, seeded randomness checked
//! statistically rather than pinned to one exact draw.

use std::collections::HashMap;
use std::path::PathBuf;

use felt_reaction_table::{Entry, RandomLike, entry_key, load_reactions, pick_reaction};
use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};

/// pick_reaction() narrowed to non-None for these tests, where `entries`
/// is always non-empty so a None pick can't actually happen - avoids
/// repeating `assert picked is not None` at every call site.
fn must_pick<'a, R: RandomLike>(
    entries: &'a [Entry],
    mood: &HashMap<String, f64>,
    last_pose: Option<&str>,
    rng: &mut R,
) -> &'a Entry {
    pick_reaction(entries, mood, last_pose, rng).expect("a pick from non-empty entries")
}

fn neutral_mood() -> HashMap<String, f64> {
    HashMap::from([
        ("energy".to_owned(), 0.5),
        ("happiness".to_owned(), 0.5),
        ("boredom".to_owned(), 0.0),
        ("affection".to_owned(), 0.5),
        ("annoyance".to_owned(), 0.0),
    ])
}

fn entries() -> Vec<Entry> {
    vec![
        json!({"pose": "soft_smile", "weight": 3}),
        json!({"pose": "curious", "weight": 3, "mood_boost": {"boredom": 2.5}}),
        json!({"pose": "pout", "weight": 2, "mood_boost": {"annoyance": 2.0}}),
    ]
}

/// Python truthiness for JSON values, for `assert row.get("pose")`-style checks.
fn py_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn shipped_reactions_path() -> PathBuf {
    // The Python default is module-relative (reactions.json at the
    // felt-animation project root); here the shipped data stays in the
    // read-only reference checkout and is never copied.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/reactions.json")
}

#[test]
fn test_empty_entries_returns_none() {
    assert!(pick_reaction(&[], &neutral_mood(), None, &mut StdRng::seed_from_u64(0)).is_none());
}

#[test]
fn test_never_repeats_the_last_pose_when_alternatives_exist() {
    let entries = entries();
    let mood = neutral_mood();
    let mut rng = StdRng::seed_from_u64(0);
    for _ in 0..50 {
        let picked = must_pick(&entries, &mood, Some("soft_smile"), &mut rng);
        assert_ne!(picked["pose"].as_str(), Some("soft_smile"));
    }
}

#[test]
fn test_falls_back_to_repeating_when_it_is_the_only_entry() {
    let single = vec![json!({"pose": "soft_smile", "weight": 1})];
    let picked = must_pick(
        &single,
        &neutral_mood(),
        Some("soft_smile"),
        &mut StdRng::seed_from_u64(0),
    );
    assert_eq!(picked["pose"].as_str(), Some("soft_smile"));
}

#[test]
fn test_high_boredom_makes_the_boosted_entry_dominate() {
    let entries = entries();
    let mut mood = neutral_mood();
    mood.insert("boredom".to_owned(), 1.0);
    let mut rng = StdRng::seed_from_u64(1);
    let picks: Vec<String> = (0..200)
        .map(|_| {
            must_pick(&entries, &mood, None, &mut rng)["pose"]
                .as_str()
                .expect("pose is a string")
                .to_owned()
        })
        .collect();
    let curious = picks.iter().filter(|p| p.as_str() == "curious").count();
    let soft_smile = picks.iter().filter(|p| p.as_str() == "soft_smile").count();
    assert!(curious > soft_smile);
}

#[test]
fn test_zero_boredom_does_not_favor_the_boredom_boosted_entry() {
    let entries = entries();
    let mut mood = neutral_mood();
    mood.insert("boredom".to_owned(), 0.0);
    let mut rng = StdRng::seed_from_u64(2);
    let picks: Vec<String> = (0..200)
        .map(|_| {
            must_pick(&entries, &mood, None, &mut rng)["pose"]
                .as_str()
                .expect("pose is a string")
                .to_owned()
        })
        .collect();
    // weight 3 (soft_smile) vs weight 3 unboosted (curious, boost is inert
    // at boredom=0) should land roughly even, not lopsided.
    let curious = picks.iter().filter(|p| p.as_str() == "curious").count();
    let soft_smile = picks.iter().filter(|p| p.as_str() == "soft_smile").count();
    assert!(curious < soft_smile * 2);
}

#[test]
fn test_all_zero_weight_still_returns_something() {
    let zero_weighted = vec![
        json!({"pose": "a", "weight": 0}),
        json!({"pose": "b", "weight": 0}),
    ];
    let picked = must_pick(
        &zero_weighted,
        &neutral_mood(),
        None,
        &mut StdRng::seed_from_u64(0),
    );
    assert!(matches!(picked["pose"].as_str(), Some("a" | "b")));
}

// -- audit 2026-09-24: seeded RNG, repeat avoidance by entry, spec fields -----

#[test]
fn test_seeded_rng_is_deterministic_and_leaves_global_random_alone() {
    let entries = entries();
    let mood = neutral_mood();
    // The Python also asserts the process-wide `random` state is untouched by
    // the seeded picks; this API has no global RNG - randomness is always
    // passed in - so that half of the check is structural here.
    let a: Vec<String> = (0..5)
        .map(|_| {
            must_pick(&entries, &mood, None, &mut StdRng::seed_from_u64(42))["pose"]
                .as_str()
                .expect("pose is a string")
                .to_owned()
        })
        .collect();
    let b: Vec<String> = (0..5)
        .map(|_| {
            must_pick(&entries, &mood, None, &mut StdRng::seed_from_u64(42))["pose"]
                .as_str()
                .expect("pose is a string")
                .to_owned()
        })
        .collect();
    assert_eq!(a, b);
}

#[test]
fn test_seeded_distribution_follows_mood_boost() {
    let entries = entries();
    let mut mood = neutral_mood();
    mood.insert("annoyance".to_owned(), 1.0);
    let mut rng = StdRng::seed_from_u64(7);
    let picks: Vec<String> = (0..3000)
        .map(|_| {
            must_pick(&entries, &mood, None, &mut rng)["pose"]
                .as_str()
                .expect("pose is a string")
                .to_owned()
        })
        .collect();
    // effective weights 3 : 3 : 2*2.0=4 -> pout ~40 %
    let pout = picks.iter().filter(|p| p.as_str() == "pout").count();
    let ratio = pout as f64 / picks.len() as f64;
    assert!((0.35..0.45).contains(&ratio), "pout share was {ratio}");
}

#[test]
fn test_repeat_avoidance_by_entry_key_keeps_same_pose_with_other_clip() {
    let entries = vec![
        json!({"pose": "grin", "tag": "emotion/laughing", "weight": 1}),
        json!({"pose": "grin", "tag": "dance", "weight": 1}),
    ];
    let mood = neutral_mood();
    let mut rng = StdRng::seed_from_u64(3);
    let last = entry_key(&entries[0]);
    for _ in 0..20 {
        let picked = must_pick(&entries, &mood, Some(last.as_str()), &mut rng);
        assert_eq!(picked["tag"].as_str(), Some("dance"));
    }
}

#[test]
fn test_spec_fields_pass_through_untouched() {
    let entries = vec![json!({
        "pose": "sleepy",
        "tag": "gesture/fan",
        "prop": "fan",
        "speak": true,
        "weight": 1
    })];
    let picked = must_pick(
        &entries,
        &neutral_mood(),
        None,
        &mut StdRng::seed_from_u64(0),
    );
    assert!(
        picked["tag"].as_str() == Some("gesture/fan")
            && picked["prop"].as_str() == Some("fan")
            && picked["speak"] == Value::Bool(true)
    );
}

#[test]
fn test_shipped_table_rows_are_well_formed() {
    let table = load_reactions(&shipped_reactions_path());
    assert!(!table.is_empty());
    for (event, rows) in &table {
        for row in rows {
            let Some(row) = row.as_object() else {
                panic!("row is not an object in {event}");
            };
            assert!(py_truthy(row.get("pose")), "{event}");
            assert!(
                row.get("weight").and_then(Value::as_f64).unwrap_or(1.0) > 0.0,
                "{event}"
            );
            for key in row.keys() {
                assert!(
                    matches!(
                        key.as_str(),
                        "pose" | "weight" | "mood_boost" | "tag" | "speak" | "prop"
                    ),
                    "{event}"
                );
            }
        }
    }
}
