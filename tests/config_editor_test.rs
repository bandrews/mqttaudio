// ABOUTME: Tests for the config editor's document model, field registry, save logic,
// ABOUTME: tone generation, key-driven editing behavior, and rendering smoke checks.

use mqttaudio::config::Config;
use mqttaudio::config_editor::app::{App, Modal};
use mqttaudio::config_editor::device_test::generate_tone;
use mqttaudio::config_editor::fields::{
    parse_field_value, path_of, ConfigDocument, FieldKind, Section, Seg, SubFieldSpec, REGISTRY,
};
use mqttaudio::config_editor::form;
use mqttaudio::config_editor::save::{save_document, save_path_candidates};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use serde_json::{json, Value};

// --- Document model ---

#[test]
fn set_creates_intermediate_objects_and_get_reads_back() {
    let mut doc = ConfigDocument::new();
    doc.set(&path_of(&["mqtt", "server"]), json!("broker.local"));
    assert_eq!(
        doc.get(&path_of(&["mqtt", "server"])),
        Some(&json!("broker.local"))
    );
    assert!(doc.get(&path_of(&["mqtt", "port"])).is_none());
}

#[test]
fn unset_removes_key_and_prunes_empty_parents() {
    let mut doc = ConfigDocument::new();
    doc.set(&path_of(&["mqtt", "tls", "ca_path"]), json!("/ca.pem"));
    doc.unset(&path_of(&["mqtt", "tls", "ca_path"]));
    // Both the emptied tls object and the emptied mqtt object are pruned.
    assert!(doc.root().as_object().unwrap().is_empty());
}

#[test]
fn unset_leaves_nonempty_parents_alone() {
    let mut doc = ConfigDocument::new();
    doc.set(&path_of(&["mqtt", "server"]), json!("a"));
    doc.set(&path_of(&["mqtt", "port"]), json!(1884));
    doc.unset(&path_of(&["mqtt", "port"]));
    assert_eq!(doc.get(&path_of(&["mqtt", "server"])), Some(&json!("a")));
}

#[test]
fn untouched_defaults_never_appear_in_output() {
    let mut doc = ConfigDocument::new();
    doc.set(&path_of(&["mqtt", "topic"]), json!("audio/#"));
    let text = doc.to_pretty_string();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let obj = parsed.as_object().unwrap();
    assert_eq!(
        obj.len(),
        1,
        "only the explicitly-set key is written: {text}"
    );
    assert_eq!(parsed["mqtt"]["topic"], json!("audio/#"));
}

#[test]
fn unknown_keys_pass_through_load_and_save() {
    let original = r#"{
        "_comment": "hand-written notes survive",
        "mqtt": { "topic": "audio/#", "_note": "nested too" }
    }"#;
    let mut doc = ConfigDocument::parse(original).unwrap();
    doc.set(&path_of(&["mqtt", "server"]), json!("broker"));
    let saved: Value = serde_json::from_str(&doc.to_pretty_string()).unwrap();
    assert_eq!(saved["_comment"], json!("hand-written notes survive"));
    assert_eq!(saved["mqtt"]["_note"], json!("nested too"));
    assert_eq!(saved["mqtt"]["server"], json!("broker"));
}

#[test]
fn document_deserializes_with_daemon_defaults() {
    let mut doc = ConfigDocument::new();
    doc.set(&path_of(&["mqtt", "topic"]), json!("audio/#"));
    let config = doc.to_config().unwrap();
    assert_eq!(config.mqtt.topic.as_deref(), Some("audio/#"));
    assert_eq!(config.audio.sample_rate, 48000);
    assert_eq!(config.audio.buffer_size, 512);
}

#[test]
fn push_appends_and_unset_removes_list_elements() {
    let mut doc = ConfigDocument::new();
    let path = path_of(&["cache", "precache"]);
    doc.push(&path, json!("a.wav"));
    doc.push(&path, json!("b.wav"));
    assert_eq!(doc.list_len(&path), 2);
    let mut item0 = path.clone();
    item0.push(Seg::Idx(0));
    doc.unset(&item0);
    assert_eq!(doc.list_len(&path), 1);
    let mut item = path.clone();
    item.push(Seg::Idx(0));
    assert_eq!(doc.get(&item), Some(&json!("b.wav")));
}

// --- Field value parsing ---

#[test]
fn parse_uint_enforces_bounds() {
    let kind = FieldKind::UInt { min: 64, max: 8192 };
    assert_eq!(parse_field_value(&kind, "512").unwrap(), Some(json!(512)));
    assert!(parse_field_value(&kind, "32").is_err());
    assert!(parse_field_value(&kind, "9000").is_err());
    assert!(parse_field_value(&kind, "abc").is_err());
}

#[test]
fn parse_float_enforces_bounds_and_finiteness() {
    let kind = FieldKind::Float {
        min: -60.0,
        max: 0.0,
    };
    assert_eq!(parse_field_value(&kind, "-1.0").unwrap(), Some(json!(-1.0)));
    assert!(parse_field_value(&kind, "1.0").is_err());
    assert!(parse_field_value(&kind, "NaN").is_err());
}

#[test]
fn parse_optional_text_empty_means_unset() {
    assert_eq!(
        parse_field_value(&FieldKind::OptionalText, "").unwrap(),
        None
    );
    assert_eq!(
        parse_field_value(&FieldKind::OptionalText, "x").unwrap(),
        Some(json!("x"))
    );
    assert!(parse_field_value(&FieldKind::Text, "").is_err());
}

#[test]
fn parse_enum_accepts_only_options() {
    let kind = FieldKind::Enum(&["text", "json"]);
    assert_eq!(
        parse_field_value(&kind, "JSON").unwrap(),
        Some(json!("json"))
    );
    assert!(parse_field_value(&kind, "xml").is_err());
}

#[test]
fn parse_channel_ref_distinguishes_index_and_alias() {
    assert_eq!(
        parse_field_value(&FieldKind::ChannelRef, "3").unwrap(),
        Some(json!(3))
    );
    assert_eq!(
        parse_field_value(&FieldKind::ChannelRef, "lfe").unwrap(),
        Some(json!("lfe"))
    );
}

#[test]
fn parse_json_value_validates() {
    assert_eq!(
        parse_field_value(&FieldKind::MapToJson, r#"{"volume": 0.5}"#).unwrap(),
        Some(json!({"volume": 0.5}))
    );
    assert!(parse_field_value(&FieldKind::MapToJson, "{nope").is_err());
}

// --- Save ---

#[test]
fn save_writes_backs_up_and_validates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mqttaudio.json");

    let mut doc = ConfigDocument::new();
    doc.set(&path_of(&["mqtt", "topic"]), json!("audio/#"));
    let outcome = save_document(&doc, &path).unwrap();
    assert!(outcome.backup.is_none());
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(first.ends_with('\n'));

    // Second save backs up the first.
    doc.set(&path_of(&["mqtt", "server"]), json!("broker"));
    let outcome = save_document(&doc, &path).unwrap();
    let backup = outcome.backup.expect("backup of existing file");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), first);

    // The saved file loads as a valid daemon config.
    let config = Config::from_file(&path).unwrap();
    assert_eq!(config.mqtt.server, "broker");
}

#[test]
fn save_refuses_invalid_config_and_leaves_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mqttaudio.json");

    let mut doc = ConfigDocument::new();
    doc.set(&path_of(&["mqtt", "topic"]), json!("audio/#"));
    save_document(&doc, &path).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    // No mqtt.topic and no http: validate() rejects it.
    let mut bad = ConfigDocument::new();
    bad.set(&path_of(&["audio", "sample_rate"]), json!(44100));
    bad.unset(&path_of(&["mqtt", "topic"]));
    let errors = save_document(&bad, &path).unwrap_err();
    assert!(
        errors.iter().any(|e| e.contains("mqtt.topic")),
        "expected the topic/http validation error, got: {errors:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn save_refuses_undeserializable_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mqttaudio.json");
    let doc = ConfigDocument::parse(r#"{ "mqtt": { "port": "not-a-number" } }"#).unwrap();
    assert!(save_document(&doc, &path).is_err());
    assert!(!path.exists());
}

#[test]
fn save_path_candidates_start_with_loaded_path() {
    let loaded = std::path::Path::new("/tmp/custom.json");
    let candidates = save_path_candidates(Some(loaded));
    assert_eq!(candidates[0], loaded);
    assert!(candidates.len() > 1);
}

// --- Tone generation ---

#[test]
fn tone_has_correct_length_fades_and_peak() {
    let tone = generate_tone(440.0, 48000, 500, 8, 0.5);
    assert_eq!(tone.len(), 24000);
    // Fade edges start and end at silence.
    assert_eq!(tone[0], 0.0);
    assert_eq!(tone[tone.len() - 1], 0.0);
    // Peak stays at or below the requested amplitude, and the body is audible.
    let peak = tone.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak <= 0.5 + 1e-6, "peak {peak}");
    assert!(peak >= 0.45, "peak {peak}");
    // Rising zero crossings approximate the requested frequency (440 Hz over
    // 0.5 s means ~220 full cycles).
    let crossings = tone
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    assert!((215..=225).contains(&crossings), "crossings {crossings}");
}

// --- Registry coverage: every config field must be editable ---

/// Collect every leaf path of a JSON value.
fn leaf_paths(value: &Value, prefix: Vec<String>, out: &mut Vec<Vec<String>>) {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.push(prefix);
            } else {
                for (k, v) in map {
                    let mut p = prefix.clone();
                    p.push(k.clone());
                    leaf_paths(v, p, out);
                }
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                out.push(prefix);
            } else {
                for (i, v) in arr.iter().enumerate() {
                    let mut p = prefix.clone();
                    p.push(i.to_string());
                    leaf_paths(v, p, out);
                }
            }
        }
        _ => out.push(prefix),
    }
}

/// Whether `kind` covers the path `rest` below a spec's own path.
fn kind_covers(kind: &FieldKind, rest: &[String]) -> bool {
    match kind {
        FieldKind::StringList | FieldKind::ChannelRefList => {
            rest.is_empty() || (rest.len() == 1 && rest[0].parse::<usize>().is_ok())
        }
        FieldKind::MapToUInt | FieldKind::MapToFloat { .. } => rest.len() <= 1,
        // Macro values are arbitrary JSON: anything below the key is covered.
        FieldKind::MapToJson => true,
        FieldKind::MemoryBudget => {
            rest.is_empty()
                || (rest.len() == 1
                    && ["mode", "fraction", "floor_mb", "ceiling_mb", "mb"]
                        .contains(&rest[0].as_str()))
        }
        FieldKind::Flag => rest.is_empty(),
        FieldKind::StructList(specs) => {
            if rest.is_empty() {
                return true;
            }
            // First segment is the list index, then a known sub-field.
            if rest[0].parse::<usize>().is_err() {
                return false;
            }
            let rest = &rest[1..];
            if rest.is_empty() {
                return true;
            }
            struct_covers(specs, rest)
        }
        // Scalar kinds cover exactly their own path.
        _ => rest.is_empty(),
    }
}

fn struct_covers(specs: &[SubFieldSpec], rest: &[String]) -> bool {
    specs
        .iter()
        .any(|s| s.key == rest[0] && kind_covers(&s.kind, &rest[1..]))
}

fn covered_by_registry(path: &[String]) -> bool {
    REGISTRY.iter().any(|spec| {
        if path.len() < spec.path.len() {
            return false;
        }
        if !spec.path.iter().zip(path.iter()).all(|(a, b)| a == b) {
            return false;
        }
        kind_covers(&spec.kind, &path[spec.path.len()..])
    })
}

#[test]
fn registry_covers_every_config_field() {
    // The default config exposes every always-serialized field; the populated
    // fixture adds the skip_serializing_if optionals and one element of every
    // collection. A new config field fails this test until the editor gets a
    // binding for it.
    let default = serde_json::to_value(Config::default()).unwrap();
    let populated: Value = json!({
        "schema_version": 1,
        "mqtt": {
            "username": "user",
            "password": "secret",
            "tls": { "ca_path": "/ca.pem" }
        },
        "audio": {
            "device": "hw:0,0",
            "channels": 8,
            "channel_aliases": { "lfe": 3 },
            "channel_volumes": { "0": 0.9 }
        },
        "cache": {
            "precache": ["a.wav"],
            "memory_budget": { "mode": "auto", "fraction": 0.4, "floor_mb": 128, "ceiling_mb": 1024 }
        },
        "security": { "allowed_directories": ["/srv/audio"] },
        "logging": { "mqtt_topic": "audio/logs" },
        "http": { "auth_token": "longenough" },
        "ducking_rules": [
            { "primary_voice": "voice", "ducked_voices": ["music"], "target_volume": 0.2, "fade_duration_ms": 300 }
        ],
        "bass_management": { "source_channels": [0, 1] },
        "inputs": [
            { "device": "mic", "volume": 0.8, "voice_id": "mic", "latency_ms": 20,
              "routes": [ { "source_channel": 0, "dest_channel": "lfe" } ] }
        ],
        "macros": { "boom": { "volume": 1.0 } }
    });
    // Sanity check: the populated fixture must itself deserialize, so the test
    // breaks when the fixture drifts from the real schema.
    let _: Config = serde_json::from_value(populated.clone()).unwrap();

    let mut paths = Vec::new();
    leaf_paths(&default, Vec::new(), &mut paths);
    leaf_paths(&populated, Vec::new(), &mut paths);

    let missing: Vec<String> = paths
        .iter()
        .filter(|p| !covered_by_registry(p))
        .map(|p| p.join("."))
        .collect();
    assert!(
        missing.is_empty(),
        "config fields without an editor binding: {missing:?}"
    );
}

// --- Key-driven editing behavior ---

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn new_app() -> App {
    App::new(ConfigDocument::new(), None)
}

fn goto_section(app: &mut App, section: Section) {
    let idx = Section::ALL.iter().position(|s| *s == section).unwrap();
    while app.section_idx < idx {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter)); // focus the form
}

#[test]
fn typing_a_value_sets_it_in_the_document() {
    let mut app = new_app();
    goto_section(&mut app, Section::Mqtt);
    app.handle_key(key(KeyCode::Enter)); // edit mqtt.server
    for c in "broker.local".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.doc.get(&path_of(&["mqtt", "server"])),
        Some(&json!("broker.local"))
    );
    assert!(app.dirty);
}

#[test]
fn toggling_a_bool_sets_the_inverse_of_the_default() {
    let mut app = new_app();
    goto_section(&mut app, Section::Cache);
    // First row of Cache is "enabled" (default true).
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.doc.get(&path_of(&["cache", "enabled"])),
        Some(&json!(false))
    );
}

#[test]
fn cycling_an_enum_walks_its_options() {
    let mut app = new_app();
    goto_section(&mut app, Section::Logging);
    // logging.level: info -> debug (next after the default).
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.doc.get(&path_of(&["logging", "level"])),
        Some(&json!("debug"))
    );
}

#[test]
fn delete_resets_a_field_to_default() {
    let mut app = new_app();
    goto_section(&mut app, Section::Mqtt);
    app.handle_key(key(KeyCode::Enter));
    for c in "x".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(app.doc.get(&path_of(&["mqtt", "server"])).is_some());
    app.handle_key(key(KeyCode::Delete));
    assert!(app.doc.get(&path_of(&["mqtt", "server"])).is_none());
}

#[test]
fn invalid_input_keeps_the_edit_open_with_an_error() {
    let mut app = new_app();
    goto_section(&mut app, Section::Audio);
    app.handle_key(key(KeyCode::Down)); // sample_rate
    app.handle_key(key(KeyCode::Enter));
    for c in "99".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(app.edit.as_ref().and_then(|e| e.error.as_ref()).is_some());
    assert!(app.doc.get(&path_of(&["audio", "sample_rate"])).is_none());
    app.handle_key(key(KeyCode::Esc));
    assert!(app.edit.is_none());
}

#[test]
fn memory_budget_cycles_through_modes() {
    let mut app = new_app();
    goto_section(&mut app, Section::Cache);
    let path = path_of(&["cache", "memory_budget"]);
    // Move to the memory_budget row by label.
    let idx = app
        .rows()
        .iter()
        .position(|r| r.label == "memory_budget")
        .unwrap();
    for _ in 0..idx {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.doc.get(&path).unwrap()["mode"], json!("auto"));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.doc.get(&path).unwrap()["mode"], json!("explicit"));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.doc.get(&path).unwrap()["mode"], json!("unlimited"));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.doc.get(&path).is_none());
}

#[test]
fn adding_a_map_entry_prompts_key_then_value() {
    let mut app = new_app();
    goto_section(&mut app, Section::Audio);
    let idx = app
        .rows()
        .iter()
        .position(|r| r.label == "channel_aliases")
        .unwrap();
    for _ in 0..idx {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter)); // open the map level
    app.handle_key(key(KeyCode::Char('a')));
    for c in "lfe".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter)); // commit key, prompts for value
    for c in "3".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.doc.get(&path_of(&["audio", "channel_aliases", "lfe"])),
        Some(&json!(3))
    );
}

#[test]
fn adding_a_ducking_rule_creates_a_deserializable_skeleton() {
    let mut app = new_app();
    goto_section(&mut app, Section::Ducking);
    app.handle_key(key(KeyCode::Enter)); // open the rule list
    app.handle_key(key(KeyCode::Char('a'))); // add a rule (opens its sub-form)
    let mut item = path_of(&["ducking_rules"]);
    item.push(Seg::Idx(0));
    assert!(app.doc.get(&item).is_some());
    // The skeleton deserializes (all DuckingRule fields are required).
    app.doc.to_config().unwrap();
}

#[test]
fn quit_with_unsaved_changes_asks_for_confirmation() {
    let mut app = new_app();
    goto_section(&mut app, Section::Mqtt);
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('x')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('q')));
    assert!(matches!(app.modal, Some(Modal::ConfirmQuit)));
    assert!(!app.quit);
    app.handle_key(key(KeyCode::Char('y')));
    assert!(app.quit);
}

// --- Rendering smoke tests ---

fn render(app: &App) {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| form::draw(frame, app)).unwrap();
}

#[test]
fn every_section_renders() {
    let mut app = new_app();
    render(&app);
    for _ in 1..Section::ALL.len() {
        app.handle_key(key(KeyCode::Down));
        render(&app);
    }
}

#[test]
fn nested_levels_render() {
    let mut app = new_app();
    goto_section(&mut app, Section::Inputs);
    app.handle_key(key(KeyCode::Enter)); // inputs list
    render(&app);
    app.handle_key(key(KeyCode::Char('a'))); // add input -> sub-form
    render(&app);
    // Open the routes list inside the input.
    let idx = app.rows().iter().position(|r| r.label == "routes").unwrap();
    for _ in 0..idx {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    render(&app);
    app.handle_key(key(KeyCode::Char('a'))); // add a route -> route sub-form
    render(&app);
}

#[test]
fn edit_prompt_and_modals_render() {
    let mut app = new_app();
    goto_section(&mut app, Section::Mqtt);
    app.handle_key(key(KeyCode::Enter)); // editing mqtt.server
    app.handle_key(key(KeyCode::Char('x')));
    render(&app);
    app.handle_key(key(KeyCode::Esc));

    app.modal = Some(Modal::Errors {
        title: "Cannot save".to_string(),
        errors: vec!["something is wrong".to_string()],
    });
    render(&app);

    app.modal = Some(Modal::ConfirmQuit);
    render(&app);

    app.modal = Some(Modal::LoadFailed {
        path: "/tmp/x.json".into(),
        error: "expected value at line 1".to_string(),
    });
    render(&app);

    app.handle_key(key(KeyCode::Char('d'))); // start from defaults
    app.handle_key(key(KeyCode::Char('s'))); // save dialog
    assert!(matches!(app.modal, Some(Modal::SaveDialog { .. })));
    render(&app);
}
