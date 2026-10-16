//! The one reaction-table test in test_react_event.py. The rest of that
//! Python file drives behavior.py's spoken-signal reactions and
//! routers/playback.py (httpx, asyncio, monkeypatch) - modules this crate
//! does not port, so those tests are out of scope here.

use std::path::PathBuf;

use felt_reaction_table::load_reactions;
use serde_json::Value;

/// Python truthiness for JSON values (`row.get("speak")` etc.).
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

#[test]
fn test_the_contract_events_speak_and_others_do_not() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/reactions.json");
    let table = load_reactions(&path);
    for event in [
        "game_start",
        "game_stop",
        "afk_end",
        "gpu_hot",
        "call_join_guess",
    ] {
        // Indexing panics when the event is missing, like Python's KeyError.
        assert!(
            table[event].iter().all(|row| py_truthy(row.get("speak"))),
            "{event}"
        );
    }
    for event in [
        "mash",
        "call_lively",
        "call_quiet",
        "afk_start",
        "music_start",
        "music_high_energy",
        "music_stop",
    ] {
        assert!(
            table
                .get(event)
                .is_some_and(|rows| rows.iter().all(|row| !py_truthy(row.get("speak")))),
            "{event}"
        );
    }
}
