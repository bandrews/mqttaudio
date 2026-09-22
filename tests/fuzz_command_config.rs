// ABOUTME: Property/fuzz tests for the untrusted JSON surfaces (parse_command, expand_macros, Config).
// ABOUTME: Asserts these never panic on arbitrary input — they must always return Ok or Err.

use mqttaudio::config::Config;
use mqttaudio::mqtt::commands::{expand_macros, parse_command};
use proptest::prelude::*;
use std::collections::HashMap;

/// A recursive strategy producing arbitrary `serde_json::Value` trees.
///
/// Covers the shapes that the example-based tests never reach: null/bool leaves,
/// extreme and fractional numbers, unicode strings, and deeply-nested arrays and
/// objects. Depth and breadth are bounded so the *generator* itself cannot blow
/// the stack — the point is to fuzz the parsers, not proptest.
fn arb_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        // Integers across the full i64/u64 range (channel indices, positions, times).
        any::<i64>().prop_map(|n| serde_json::json!(n)),
        any::<u64>().prop_map(|n| serde_json::json!(n)),
        // Finite floats only: serde_json cannot represent NaN/Inf as a Value, so
        // those are unrepresentable in this leaf; the string-fuzz tests cover the
        // textual "NaN"/"Infinity" rejection path instead.
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| serde_json::json!(f)),
        ".*".prop_map(serde_json::Value::String),
    ];

    leaf.prop_recursive(
        5,  // up to 5 levels deep
        64, // up to 64 total nodes
        10, // up to 10 children per collection
        |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..10).prop_map(serde_json::Value::Array),
                prop::collection::hash_map(".*", inner, 0..10)
                    .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
            ]
        },
    )
}

/// Command names the dispatcher recognises, plus a couple of unknowns, so the
/// structured-payload strategies exercise the real per-command parsing arms
/// (channel maps, selectors, numeric fields) rather than only the unknown arm.
fn arb_command_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("play".to_string()),
        Just("soundPlay".to_string()),
        Just("stop".to_string()),
        Just("stopall".to_string()),
        Just("volume".to_string()),
        Just("seek".to_string()),
        Just("speed".to_string()),
        Just("precache".to_string()),
        Just("cache_clear".to_string()),
        Just("cache_invalidate".to_string()),
        Just("voice_stop".to_string()),
        Just("voice_fade_out".to_string()),
        Just("voice_volume".to_string()),
        Just("input_volume".to_string()),
        Just("input_mute".to_string()),
        ".*", // arbitrary (mostly unknown) command names
    ]
}

proptest! {
    // Raw arbitrary strings: exercises the serde_json::from_str entry point of
    // parse_command, including malformed/non-JSON bytes and partial documents.
    #[test]
    fn parse_command_never_panics_on_arbitrary_string(s in ".*") {
        match parse_command(&s) {
            Ok(_) | Err(_) => {}
        }
    }

    // Arbitrary well-formed JSON values, serialized: reaches get_params() and the
    // per-command from_value() parsing for non-string/array/object roots.
    #[test]
    fn parse_command_never_panics_on_arbitrary_json(v in arb_json()) {
        let json = serde_json::to_string(&v).expect("serializing a Value cannot fail");
        match parse_command(&json) {
            Ok(_) | Err(_) => {}
        }
    }

    // A real command envelope with a recognised command name and hostile params:
    // drives the per-command parsing arms (channel_map, selectors, extreme numerics).
    #[test]
    fn parse_command_never_panics_on_structured_envelope(
        command in arb_command_name(),
        params in arb_json(),
        nested in any::<bool>(),
    ) {
        let envelope = if nested {
            serde_json::json!({ "command": command, "message": params })
        } else {
            // Flattened form: merge params into the root next to "command".
            let mut obj = serde_json::Map::new();
            obj.insert("command".to_string(), serde_json::json!(command));
            if let serde_json::Value::Object(p) = params {
                for (k, val) in p {
                    if k != "command" {
                        obj.insert(k, val);
                    }
                }
            }
            serde_json::Value::Object(obj)
        };
        let json = serde_json::to_string(&envelope).expect("serializing a Value cannot fail");
        match parse_command(&json) {
            Ok(_) | Err(_) => {}
        }
    }

    // expand_macros over an arbitrary input string and an arbitrary macro table.
    // Exercises macro-name resolution, forward-precedence merging, and the
    // non-object / non-string-macro early-return branches.
    #[test]
    fn expand_macros_never_panics(
        input in ".*",
        macro_entries in prop::collection::vec((".*", arb_json()), 0..8),
    ) {
        let macros: HashMap<String, serde_json::Value> = macro_entries.into_iter().collect();
        match expand_macros(&input, &macros) {
            Ok(_) | Err(_) => {}
        }
    }

    // expand_macros where the input is a real command object that may reference
    // generated macro names (single string or array), so the merge path runs.
    #[test]
    fn expand_macros_never_panics_with_macro_refs(
        macro_ref in prop_oneof![
            ".*".prop_map(serde_json::Value::String),
            prop::collection::vec(".*", 0..5)
                .prop_map(|v| serde_json::Value::Array(
                    v.into_iter().map(serde_json::Value::String).collect()
                )),
            arb_json(),
        ],
        extra in arb_json(),
        macro_entries in prop::collection::vec((".*", arb_json()), 0..8),
    ) {
        let mut obj = serde_json::Map::new();
        obj.insert("macro".to_string(), macro_ref);
        if let serde_json::Value::Object(p) = extra {
            for (k, v) in p {
                obj.insert(k, v);
            }
        }
        let input = serde_json::to_string(&serde_json::Value::Object(obj))
            .expect("serializing a Value cannot fail");
        let macros: HashMap<String, serde_json::Value> = macro_entries.into_iter().collect();
        match expand_macros(&input, &macros) {
            Ok(_) | Err(_) => {}
        }
    }

    // Config parsed from arbitrary strings: the from_str path config::from_file uses.
    #[test]
    fn config_from_arbitrary_string_never_panics(s in ".*") {
        match serde_json::from_str::<Config>(&s) {
            Ok(_) | Err(_) => {}
        }
    }

    // Config parsed from arbitrary well-formed JSON: exercises the serde derive
    // for every nested config section (audio/cache/http/bass/inputs/macros/...).
    #[test]
    fn config_from_arbitrary_json_never_panics(v in arb_json()) {
        let json = serde_json::to_string(&v).expect("serializing a Value cannot fail");
        match serde_json::from_str::<Config>(&json) {
            Ok(_) | Err(_) => {}
        }
    }
}
