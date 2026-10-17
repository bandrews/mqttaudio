// ABOUTME: Application state and key handling for the interactive config editor.
// ABOUTME: Drives section navigation, field editing, modals, and live device tests.

use super::device_test::{
    start_input_test, start_output_test, InputTestSession, OutputTestSession, BASS_TONE_HZ,
    TEST_TONE_HZ, TONE_DURATION_MS,
};
use super::fields::{
    default_config_value, display_value, parse_field_value, path_of, section_fields,
    ConfigDocument, FieldKind, Section, Seg, StructListMeta, SubFieldSpec,
};
use super::save::{save_document, save_path_candidates};
use crate::config::{Config, MemoryBudget};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Instant;

/// Which pane has keyboard focus.
#[derive(Clone, Copy, PartialEq)]
pub enum Pane {
    Sidebar,
    Form,
}

/// What a nested editing level shows.
pub enum LevelKind {
    /// A top-level section's form.
    Section(Section),
    /// A struct's fields (one ducking rule, one input, one route).
    SubForm {
        specs: &'static [SubFieldSpec],
        defaults: Value,
    },
    /// A list of scalar values: strings (precache), channel refs
    /// (bass_management.source_channels), or voice names (ducked_voices).
    ScalarList { elem: FieldKind },
    /// A map with values of `value_kind` (aliases, volumes).
    Map { value_kind: FieldKind },
    /// One macro's parameters: known command parameters edit with their real
    /// kinds, anything else as raw JSON.
    MacroForm,
    /// A list of structs (ducking_rules, inputs, routes).
    StructList { meta: &'static StructListMeta },
}

/// One level of the editing stack: a form or collection at a JSON path.
pub struct Level {
    pub title: String,
    pub path: Vec<Seg>,
    pub kind: LevelKind,
    pub cursor: usize,
    /// Explains the feature this level edits; shown above its rows.
    pub intro: String,
}

/// One selectable row of the current level.
pub struct Row {
    pub label: String,
    pub path: Vec<Seg>,
    pub kind: FieldKind,
    pub help: String,
    pub display: String,
    pub is_set: bool,
}

/// What an in-progress text edit will do when committed.
#[derive(Clone)]
pub enum EditTarget {
    /// Set (or unset, for optional kinds) the field at `path`.
    Field { path: Vec<Seg>, kind: FieldKind },
    /// Append a new element to the list at `base`.
    NewListItem { base: Vec<Seg>, kind: FieldKind },
    /// First step of adding a map entry: the key. Commits into a `MapValue` edit.
    MapKey {
        base: Vec<Seg>,
        value_kind: FieldKind,
    },
    /// Set the map value at `path` (an existing or just-named key).
    MapValue { path: Vec<Seg>, kind: FieldKind },
}

/// An in-progress single-line text edit.
pub struct EditState {
    pub label: String,
    pub buffer: String,
    pub cursor: usize,
    pub target: EditTarget,
    pub error: Option<String>,
}

/// One entry in the device picker.
pub struct PickerItem {
    /// Device name to store in the config; None = system default.
    pub name: Option<String>,
    pub detail: String,
}

/// One entry in a choice picker.
pub struct Choice {
    /// Text committed through the normal field-parsing path when chosen.
    pub insert_text: String,
    pub label: String,
    pub detail: String,
    /// A "type it yourself" entry: choosing it opens a free-text edit.
    pub custom: bool,
    /// For key pickers: the kind of value this key takes (overrides the
    /// target's value kind).
    pub value_kind: Option<FieldKind>,
}

impl Choice {
    fn new(insert_text: &str, label: &str, detail: &str) -> Self {
        Self {
            insert_text: insert_text.to_string(),
            label: label.to_string(),
            detail: detail.to_string(),
            custom: false,
            value_kind: None,
        }
    }

    fn custom(label: &str, detail: &str) -> Self {
        Self {
            insert_text: String::new(),
            label: label.to_string(),
            detail: detail.to_string(),
            custom: true,
            value_kind: None,
        }
    }
}

/// Live output test screen state.
pub struct OutputTestState {
    pub session: Option<OutputTestSession>,
    pub error: Option<String>,
    pub cursor: usize,
    /// All-channels sweep: (next channel to play, time of the last play).
    pub sweep: Option<(usize, Instant)>,
    pub device_label: String,
    /// idx -> alias name, from audio.channel_aliases.
    pub aliases: Vec<Option<String>>,
    /// Reopen this picker when the test screen closes.
    pub return_to_picker: Option<(bool, Vec<Seg>)>,
}

/// Live input level-meter screen state.
pub struct InputTestState {
    pub session: Option<InputTestSession>,
    pub error: Option<String>,
    pub device_label: String,
    pub return_to_picker: Option<(bool, Vec<Seg>)>,
}

/// A blocking overlay.
pub enum Modal {
    DevicePicker {
        output: bool,
        items: Vec<PickerItem>,
        cursor: usize,
        target: Vec<Seg>,
    },
    /// A list of values to pick from; commits through the edit pipeline.
    ChoicePicker {
        title: String,
        /// One dimmed context line under the list ("" = none).
        hint: String,
        /// Label of the free-text edit a custom choice opens.
        edit_label: String,
        items: Vec<Choice>,
        cursor: usize,
        target: EditTarget,
    },
    OutputTest(OutputTestState),
    InputTest(InputTestState),
    SaveDialog {
        buffer: String,
        cursor: usize,
        candidates: Vec<PathBuf>,
    },
    Errors {
        title: String,
        errors: Vec<String>,
    },
    ConfirmQuit,
    LoadFailed {
        path: PathBuf,
        error: String,
    },
}

/// The whole editor's state.
pub struct App {
    pub doc: ConfigDocument,
    pub defaults: Value,
    pub loaded_from: Option<PathBuf>,
    pub dirty: bool,
    pub section_idx: usize,
    pub pane: Pane,
    pub levels: Vec<Level>,
    pub edit: Option<EditState>,
    pub modal: Option<Modal>,
    pub status: String,
    pub quit: bool,
}

impl App {
    pub fn new(doc: ConfigDocument, loaded_from: Option<PathBuf>) -> Self {
        let status = match &loaded_from {
            Some(p) => format!("Editing {}", p.display()),
            None => "New configuration (no file found)".to_string(),
        };
        Self {
            doc,
            defaults: default_config_value(),
            loaded_from,
            dirty: false,
            section_idx: 0,
            pane: Pane::Sidebar,
            levels: vec![Level {
                title: Section::ALL[0].title().to_string(),
                path: Vec::new(),
                kind: LevelKind::Section(Section::ALL[0]),
                cursor: 0,
                intro: Section::ALL[0].intro().to_string(),
            }],
            edit: None,
            modal: None,
            status,
            quit: false,
        }
    }

    pub fn current_section(&self) -> Section {
        Section::ALL[self.section_idx]
    }

    fn reset_levels(&mut self) {
        let section = self.current_section();
        self.levels = vec![Level {
            title: section.title().to_string(),
            path: Vec::new(),
            kind: LevelKind::Section(section),
            cursor: 0,
            intro: section.intro().to_string(),
        }];
    }

    fn level(&self) -> &Level {
        self.levels.last().expect("levels is never empty")
    }

    fn level_mut(&mut self) -> &mut Level {
        self.levels.last_mut().expect("levels is never empty")
    }

    /// The breadcrumb title for the content pane.
    pub fn breadcrumb(&self) -> String {
        self.levels
            .iter()
            .map(|l| l.title.as_str())
            .collect::<Vec<_>>()
            .join(" > ")
    }

    /// The default value at a static path under `Config::default()`.
    fn default_at(&self, path: &[Seg]) -> Value {
        let mut cur = &self.defaults;
        for seg in path {
            match seg {
                Seg::Key(k) => match cur.as_object().and_then(|o| o.get(k)) {
                    Some(v) => cur = v,
                    None => return Value::Null,
                },
                Seg::Idx(_) => return Value::Null,
            }
        }
        cur.clone()
    }

    /// The effective value at `path` (explicit or default) and whether it was
    /// explicitly set.
    fn effective(&self, path: &[Seg]) -> (Value, bool) {
        match self.doc.get(path) {
            Some(v) => (v.clone(), true),
            None => (self.default_at(path), false),
        }
    }

    /// Compute the rows of the current level.
    pub fn rows(&self) -> Vec<Row> {
        let level = self.level();
        match &level.kind {
            LevelKind::Section(section) => {
                let mut rows = Vec::new();
                for spec in section_fields(*section) {
                    let path = path_of(spec.path);
                    let (value, is_set) = self.effective(&path);
                    rows.push(Row {
                        label: spec.label.to_string(),
                        display: display_value(&spec.kind, &value),
                        path,
                        kind: spec.kind,
                        help: spec.help.to_string(),
                        is_set,
                    });
                    // The memory budget's mode-specific parameters appear as
                    // extra rows right below it.
                    if spec.kind == FieldKind::MemoryBudget {
                        self.push_memory_budget_rows(&mut rows, spec.path);
                    }
                }
                rows
            }
            LevelKind::SubForm { specs, defaults } => specs
                .iter()
                .map(|spec| {
                    let mut path = level.path.clone();
                    path.push(Seg::Key(spec.key.to_string()));
                    let (value, is_set) = match self.doc.get(&path) {
                        Some(v) => (v.clone(), true),
                        None => (
                            defaults
                                .as_object()
                                .and_then(|o| o.get(spec.key))
                                .cloned()
                                .unwrap_or(Value::Null),
                            false,
                        ),
                    };
                    Row {
                        label: spec.label.to_string(),
                        display: display_value(&spec.kind, &value),
                        path,
                        kind: spec.kind,
                        help: spec.help.to_string(),
                        is_set,
                    }
                })
                .collect(),
            LevelKind::ScalarList { elem } => self
                .list_values(&level.path)
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let mut path = level.path.clone();
                    path.push(Seg::Idx(i));
                    Row {
                        label: format!("{}", i + 1),
                        display: display_value(elem, v),
                        path,
                        kind: *elem,
                        help: scalar_item_help(elem).to_string(),
                        is_set: true,
                    }
                })
                .collect(),
            LevelKind::Map { value_kind } => {
                let mut rows: Vec<Row> = Vec::new();
                if let Some(Value::Object(map)) = self.doc.get(&level.path) {
                    let mut keys: Vec<&String> = map.keys().collect();
                    keys.sort();
                    for key in keys {
                        let mut path = level.path.clone();
                        path.push(Seg::Key(key.clone()));
                        let value = &map[key];
                        let display = match value_kind {
                            FieldKind::MapToJson => {
                                serde_json::to_string(value).unwrap_or_default()
                            }
                            _ => display_value(value_kind, value),
                        };
                        rows.push(Row {
                            label: key.clone(),
                            display,
                            path,
                            kind: *value_kind,
                            help: map_item_help(value_kind).to_string(),
                            is_set: true,
                        });
                    }
                }
                rows
            }
            LevelKind::MacroForm => {
                let mut rows: Vec<Row> = Vec::new();
                if let Some(Value::Object(map)) = self.doc.get(&level.path) {
                    let mut keys: Vec<&String> = map.keys().collect();
                    keys.sort();
                    for key in keys {
                        let mut path = level.path.clone();
                        path.push(Seg::Key(key.clone()));
                        let value = &map[key];
                        let (kind, help) = match super::fields::macro_param_spec(key) {
                            Some(spec) => (spec.kind, spec.help.to_string()),
                            None => (
                                FieldKind::MapToJson,
                                "Custom parameter; the value is raw JSON.".to_string(),
                            ),
                        };
                        let display = match kind {
                            FieldKind::MapToJson => {
                                serde_json::to_string(value).unwrap_or_default()
                            }
                            _ => display_value(&kind, value),
                        };
                        rows.push(Row {
                            label: key.clone(),
                            display,
                            path,
                            kind,
                            help,
                            is_set: true,
                        });
                    }
                }
                rows
            }
            LevelKind::StructList { meta } => self
                .list_values(&level.path)
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let mut path = level.path.clone();
                    path.push(Seg::Idx(i));
                    Row {
                        label: format!("{} {}", meta.item_noun, i + 1),
                        display: (meta.summarize)(v),
                        path,
                        kind: FieldKind::StructList(meta),
                        help: format!(
                            "One {} in this list. Enter opens its fields.",
                            meta.item_noun
                        ),
                        is_set: true,
                    }
                })
                .collect(),
        }
    }

    fn list_values(&self, path: &[Seg]) -> Vec<Value> {
        match self.doc.get(path) {
            Some(Value::Array(arr)) => arr.clone(),
            _ => Vec::new(),
        }
    }

    fn push_memory_budget_rows(&self, rows: &mut Vec<Row>, base: &[&str]) {
        let base_path = path_of(base);
        let Some(Value::Object(budget)) = self.doc.get(&base_path) else {
            return;
        };
        let auto_defaults =
            serde_json::to_value(MemoryBudget::default_auto()).unwrap_or(Value::Null);
        let mode = budget.get("mode").and_then(|m| m.as_str()).unwrap_or("");
        let sub: &[(&str, FieldKind, &str)] = match mode {
            "auto" => &[
                (
                    "fraction",
                    FieldKind::Float { min: 0.0, max: 1.0 },
                    "Fraction of available RAM to target. Default 0.4.",
                ),
                (
                    "floor_mb",
                    FieldKind::UInt {
                        min: 1,
                        max: u32::MAX as u64,
                    },
                    "Minimum cap in MiB, so small assets still cache on a tiny box. Default 128.",
                ),
                (
                    "ceiling_mb",
                    FieldKind::UInt {
                        min: 1,
                        max: u32::MAX as u64,
                    },
                    "Maximum cap in MiB, so the daemon never camps all RAM. Default 1024.",
                ),
            ],
            "explicit" => &[(
                "mb",
                FieldKind::UInt {
                    min: 1,
                    max: u32::MAX as u64,
                },
                "Fixed memory cache cap in MiB.",
            )],
            _ => &[],
        };
        for (key, kind, help) in sub {
            let mut path = base_path.clone();
            path.push(Seg::Key(key.to_string()));
            let (value, is_set) = match budget.get(*key) {
                Some(v) => (v.clone(), true),
                None => (
                    auto_defaults
                        .as_object()
                        .and_then(|o| o.get(*key))
                        .cloned()
                        .unwrap_or(Value::Null),
                    false,
                ),
            };
            rows.push(Row {
                label: format!("memory_budget.{}", key),
                display: display_value(kind, &value),
                path,
                kind: *kind,
                help: help.to_string(),
                is_set,
            });
        }
    }

    /// Advance to the next state for one key press.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.request_quit();
            return;
        }
        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }
        if self.edit.is_some() {
            self.handle_edit_key(key);
            return;
        }
        self.handle_normal_key(key);
    }

    fn request_quit(&mut self) {
        if self.dirty {
            self.modal = Some(Modal::ConfirmQuit);
        } else {
            self.quit = true;
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('s') => self.open_save_dialog(),
            KeyCode::Tab => {
                self.pane = match self.pane {
                    Pane::Sidebar => Pane::Form,
                    Pane::Form => Pane::Sidebar,
                };
            }
            _ => match self.pane {
                Pane::Sidebar => self.handle_sidebar_key(key),
                Pane::Form => self.handle_form_key(key),
            },
        }
    }

    fn handle_sidebar_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up if self.section_idx > 0 => {
                self.section_idx -= 1;
                self.reset_levels();
            }
            KeyCode::Down if self.section_idx + 1 < Section::ALL.len() => {
                self.section_idx += 1;
                self.reset_levels();
            }
            KeyCode::Right | KeyCode::Enter => self.pane = Pane::Form,
            _ => {}
        }
    }

    fn handle_form_key(&mut self, key: KeyEvent) {
        let row_count = self.rows().len();
        match key.code {
            KeyCode::Up => {
                let level = self.level_mut();
                level.cursor = level.cursor.saturating_sub(1);
            }
            KeyCode::Down => {
                let level = self.level_mut();
                if row_count > 0 && level.cursor + 1 < row_count {
                    level.cursor += 1;
                }
            }
            KeyCode::Left | KeyCode::Esc => {
                if self.levels.len() > 1 {
                    self.levels.pop();
                } else {
                    self.pane = Pane::Sidebar;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_row(),
            KeyCode::Delete | KeyCode::Char('u') => self.unset_row(),
            KeyCode::Char('d') if self.level_is_collection() => {
                self.unset_row();
            }
            KeyCode::Char('a') => self.add_to_collection(),
            _ => {}
        }
    }

    pub fn level_is_collection(&self) -> bool {
        matches!(
            self.level().kind,
            LevelKind::ScalarList { .. }
                | LevelKind::Map { .. }
                | LevelKind::MacroForm
                | LevelKind::StructList { .. }
        )
    }

    /// Enter/space on the highlighted row.
    fn activate_row(&mut self) {
        let rows = self.rows();
        let cursor = self.level().cursor;
        let Some(row) = rows.get(cursor) else {
            // Empty collection: Enter adds the first entry.
            if self.level_is_collection() {
                self.add_to_collection();
            }
            return;
        };
        let in_map = matches!(self.level().kind, LevelKind::Map { .. });
        match row.kind {
            FieldKind::Bool => {
                let (value, _) = self.effective(&row.path);
                let current = value.as_bool().unwrap_or(false);
                self.doc.set(&row.path, Value::Bool(!current));
                self.dirty = true;
            }
            FieldKind::Flag => {
                if self.doc.get(&row.path).is_some() {
                    self.doc.unset(&row.path);
                } else {
                    self.doc
                        .set(&row.path, Value::Object(serde_json::Map::new()));
                }
                self.dirty = true;
            }
            FieldKind::Enum(options) => {
                let (value, _) = self.effective(&row.path);
                let current = value.as_str().map(|s| s.to_string());
                self.open_choice_picker(
                    row.label.clone(),
                    String::new(),
                    row.label.clone(),
                    enum_choices(options),
                    current,
                    self.row_target(row),
                );
            }
            FieldKind::ChannelRef => {
                let current = match self.effective(&row.path).0 {
                    Value::String(s) => Some(s),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                };
                let items = self.channel_choices();
                self.open_choice_picker(
                    "Select channel".to_string(),
                    CHANNEL_PICKER_HINT.to_string(),
                    "channel (number or alias)".to_string(),
                    items,
                    current,
                    self.row_target(row),
                );
            }
            FieldKind::VoiceRef => {
                let current = self.effective(&row.path).0.as_str().map(|s| s.to_string());
                let items = self.voice_choices();
                self.open_choice_picker(
                    "Select voice".to_string(),
                    VOICE_PICKER_HINT.to_string(),
                    "voice name".to_string(),
                    items,
                    current,
                    self.row_target(row),
                );
            }
            FieldKind::MemoryBudget => {
                self.cycle_memory_budget(&row.path);
            }
            FieldKind::OutputDevice => self.open_device_picker(true, row.path.clone()),
            FieldKind::InputDevice => self.open_device_picker(false, row.path.clone()),
            FieldKind::StringList => self.push_level(Level {
                title: row.label.clone(),
                path: row.path.clone(),
                kind: LevelKind::ScalarList {
                    elem: FieldKind::Text,
                },
                cursor: 0,
                intro: row.help.clone(),
            }),
            FieldKind::ChannelRefList => self.push_level(Level {
                title: row.label.clone(),
                path: row.path.clone(),
                kind: LevelKind::ScalarList {
                    elem: FieldKind::ChannelRef,
                },
                cursor: 0,
                intro: row.help.clone(),
            }),
            FieldKind::VoiceRefList => self.push_level(Level {
                title: row.label.clone(),
                path: row.path.clone(),
                kind: LevelKind::ScalarList {
                    elem: FieldKind::VoiceRef,
                },
                cursor: 0,
                intro: row.help.clone(),
            }),
            FieldKind::MapToUInt => self.push_level(Level {
                title: row.label.clone(),
                path: row.path.clone(),
                kind: LevelKind::Map {
                    value_kind: FieldKind::UInt { min: 0, max: 65535 },
                },
                cursor: 0,
                intro: row.help.clone(),
            }),
            FieldKind::MapToFloat { min, max } => self.push_level(Level {
                title: row.label.clone(),
                path: row.path.clone(),
                kind: LevelKind::Map {
                    value_kind: FieldKind::Float { min, max },
                },
                cursor: 0,
                intro: row.help.clone(),
            }),
            // A macro entry inside the macros map: open its guided form.
            FieldKind::MapToJson if in_map => self.push_level(Level {
                title: row.label.clone(),
                path: row.path.clone(),
                kind: LevelKind::MacroForm,
                cursor: 0,
                intro: format!(
                    "Parameters merged into commands that use \"macro\": \"{}\". \
                     Command parameters override macro parameters.",
                    row.label
                ),
            }),
            // A raw-JSON macro parameter: edit the value as text.
            FieldKind::MapToJson if matches!(self.level().kind, LevelKind::MacroForm) => {
                self.start_field_edit(row)
            }
            // The macros field itself: open the map of macros.
            FieldKind::MapToJson => self.push_level(Level {
                title: row.label.clone(),
                path: row.path.clone(),
                kind: LevelKind::Map {
                    value_kind: FieldKind::MapToJson,
                },
                cursor: 0,
                intro: row.help.clone(),
            }),
            FieldKind::StructList(meta) => {
                if matches!(self.level().kind, LevelKind::StructList { .. }) {
                    // A list item row: open its sub-form.
                    self.open_subform(row.path.clone(), meta, row.label.clone());
                } else {
                    // A field row pointing at a struct list: open the list.
                    self.push_level(Level {
                        title: row.label.clone(),
                        path: row.path.clone(),
                        kind: LevelKind::StructList { meta },
                        cursor: 0,
                        intro: row.help.clone(),
                    });
                }
            }
            _ => self.start_field_edit(row),
        }
    }

    fn start_field_edit(&mut self, row: &Row) {
        let buffer = match row.kind {
            FieldKind::Secret => String::new(),
            FieldKind::MapToJson => self
                .doc
                .get(&row.path)
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default(),
            _ => match self.doc.get(&row.path) {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            },
        };
        let target = if matches!(self.level().kind, LevelKind::Map { .. }) {
            EditTarget::MapValue {
                path: row.path.clone(),
                kind: row.kind,
            }
        } else {
            EditTarget::Field {
                path: row.path.clone(),
                kind: row.kind,
            }
        };
        self.edit = Some(EditState {
            label: row.label.clone(),
            cursor: buffer.chars().count(),
            buffer,
            target,
            error: None,
        });
    }

    fn cycle_memory_budget(&mut self, path: &[Seg]) {
        let mode = self
            .doc
            .get(path)
            .and_then(|v| v.get("mode"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string());
        match mode.as_deref() {
            None => {
                self.doc.set(path, serde_json::json!({ "mode": "auto" }));
            }
            Some("auto") => {
                self.doc
                    .set(path, serde_json::json!({ "mode": "explicit", "mb": 512 }));
            }
            Some("explicit") => {
                self.doc
                    .set(path, serde_json::json!({ "mode": "unlimited" }));
            }
            _ => self.doc.unset(path),
        }
        self.dirty = true;
    }

    fn push_level(&mut self, level: Level) {
        self.levels.push(level);
    }

    fn open_subform(&mut self, path: Vec<Seg>, meta: &'static StructListMeta, title: String) {
        self.push_level(Level {
            title,
            path,
            kind: LevelKind::SubForm {
                specs: meta.specs,
                defaults: (meta.item_defaults)(),
            },
            cursor: 0,
            intro: String::new(),
        });
    }

    /// Del/'u' on the highlighted row: unset a field or remove a collection entry.
    fn unset_row(&mut self) {
        let rows = self.rows();
        let cursor = self.level().cursor;
        let Some(row) = rows.get(cursor) else {
            return;
        };
        self.doc.unset(&row.path);
        self.dirty = true;
        let row_count = self.rows().len();
        let level = self.level_mut();
        if level.cursor >= row_count && level.cursor > 0 {
            level.cursor -= 1;
        }
    }

    /// 'a' in a collection level: add an entry.
    fn add_to_collection(&mut self) {
        enum Add {
            Item(FieldKind, &'static str),
            MapEntry(FieldKind),
            MacroParam,
            Struct(&'static StructListMeta),
            None,
        }
        let path = self.level().path.clone();
        let action = match &self.level().kind {
            LevelKind::ScalarList { elem } => Add::Item(
                *elem,
                match elem {
                    FieldKind::ChannelRef => "new channel (number or alias)",
                    FieldKind::VoiceRef => "new voice name",
                    _ => "new entry",
                },
            ),
            LevelKind::Map { value_kind } => Add::MapEntry(*value_kind),
            LevelKind::MacroForm => Add::MacroParam,
            LevelKind::StructList { meta } => Add::Struct(meta),
            _ => Add::None,
        };
        match action {
            Add::Item(FieldKind::ChannelRef, label) => {
                let items = self.channel_choices();
                self.open_choice_picker(
                    "Add channel".to_string(),
                    CHANNEL_PICKER_HINT.to_string(),
                    label.to_string(),
                    items,
                    None,
                    EditTarget::NewListItem {
                        base: path,
                        kind: FieldKind::ChannelRef,
                    },
                );
            }
            Add::Item(FieldKind::VoiceRef, label) => {
                let items = self.voice_choices();
                self.open_choice_picker(
                    "Add voice".to_string(),
                    VOICE_PICKER_HINT.to_string(),
                    label.to_string(),
                    items,
                    None,
                    EditTarget::NewListItem {
                        base: path,
                        kind: FieldKind::VoiceRef,
                    },
                );
            }
            Add::Item(kind, label) => {
                self.edit = Some(EditState {
                    label: label.to_string(),
                    buffer: String::new(),
                    cursor: 0,
                    target: EditTarget::NewListItem { base: path, kind },
                    error: None,
                });
            }
            // Channel-volume keys are channel references: offer the picker.
            Add::MapEntry(value_kind) if path == path_of(&["audio", "channel_volumes"]) => {
                let items = self.channel_choices();
                self.open_choice_picker(
                    "Add channel".to_string(),
                    CHANNEL_PICKER_HINT.to_string(),
                    "channel (number or alias)".to_string(),
                    items,
                    None,
                    EditTarget::MapKey {
                        base: path,
                        value_kind,
                    },
                );
            }
            Add::MapEntry(value_kind) => {
                self.edit = Some(EditState {
                    label: "new key".to_string(),
                    buffer: String::new(),
                    cursor: 0,
                    target: EditTarget::MapKey {
                        base: path,
                        value_kind,
                    },
                    error: None,
                });
            }
            Add::MacroParam => {
                let existing = self.doc.get(&path).cloned().unwrap_or(Value::Null);
                self.open_choice_picker(
                    "Add parameter".to_string(),
                    "Play-command parameters; macros may also carry parameters of other commands."
                        .to_string(),
                    "new parameter name".to_string(),
                    macro_param_choices(&existing),
                    None,
                    EditTarget::MapKey {
                        base: path,
                        value_kind: FieldKind::MapToJson,
                    },
                );
            }
            Add::Struct(meta) => {
                self.doc.push(&path, (meta.skeleton)());
                self.dirty = true;
                let idx = self.doc.list_len(&path) - 1;
                let title = format!("{} {}", meta.item_noun, idx + 1);
                let mut item_path = path;
                item_path.push(Seg::Idx(idx));
                self.open_subform(item_path, meta, title);
            }
            Add::None => {}
        }
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.edit = None,
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Char(c) => {
                let byte = byte_index(&edit.buffer, edit.cursor);
                edit.buffer.insert(byte, c);
                edit.cursor += 1;
            }
            KeyCode::Backspace if edit.cursor > 0 => {
                edit.cursor -= 1;
                let byte = byte_index(&edit.buffer, edit.cursor);
                edit.buffer.remove(byte);
            }
            KeyCode::Delete if edit.cursor < edit.buffer.chars().count() => {
                let byte = byte_index(&edit.buffer, edit.cursor);
                edit.buffer.remove(byte);
            }
            KeyCode::Left => edit.cursor = edit.cursor.saturating_sub(1),
            KeyCode::Right if edit.cursor < edit.buffer.chars().count() => {
                edit.cursor += 1;
            }
            KeyCode::Home => edit.cursor = 0,
            KeyCode::End => edit.cursor = edit.buffer.chars().count(),
            _ => {}
        }
    }

    fn commit_edit(&mut self) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        match edit.target {
            EditTarget::Field { ref path, ref kind } => {
                match parse_field_value(kind, &edit.buffer) {
                    Ok(Some(v)) => {
                        self.doc.set(path, v);
                        self.dirty = true;
                    }
                    Ok(None) => {
                        self.doc.unset(path);
                        self.dirty = true;
                    }
                    Err(e) => {
                        self.edit = Some(EditState {
                            error: Some(e),
                            ..edit
                        });
                    }
                }
            }
            EditTarget::NewListItem { ref base, ref kind } => {
                match parse_field_value(kind, &edit.buffer) {
                    Ok(Some(v)) => {
                        self.doc.push(base, v);
                        self.dirty = true;
                        let count = self.doc.list_len(base);
                        self.level_mut().cursor = count.saturating_sub(1);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.edit = Some(EditState {
                            error: Some(e),
                            ..edit
                        });
                    }
                }
            }
            EditTarget::MapKey {
                ref base,
                ref value_kind,
            } => {
                let key = edit.buffer.trim().to_string();
                if key.is_empty() {
                    self.edit = Some(EditState {
                        error: Some("a key is required".to_string()),
                        ..edit
                    });
                    return;
                }
                let mut path = base.clone();
                path.push(Seg::Key(key.clone()));
                if self.doc.get(&path).is_some() {
                    self.edit = Some(EditState {
                        error: Some(format!("'{}' already exists", key)),
                        ..edit
                    });
                    return;
                }
                // A new name in the macros map creates an empty macro and
                // opens its parameter form instead of asking for raw JSON.
                if *base == path_of(&["macros"]) {
                    self.doc.set(&path, Value::Object(serde_json::Map::new()));
                    self.dirty = true;
                    let intro = format!(
                        "Parameters merged into commands that use \"macro\": \"{}\". \
                         Command parameters override macro parameters.",
                        key
                    );
                    self.push_level(Level {
                        title: key,
                        path,
                        kind: LevelKind::MacroForm,
                        cursor: 0,
                        intro,
                    });
                    return;
                }
                self.edit = Some(EditState {
                    label: format!("value for {}", key),
                    buffer: String::new(),
                    cursor: 0,
                    target: EditTarget::MapValue {
                        path,
                        kind: *value_kind,
                    },
                    error: None,
                });
            }
            EditTarget::MapValue { ref path, ref kind } => {
                match parse_field_value(kind, &edit.buffer) {
                    Ok(Some(v)) => {
                        self.doc.set(path, v);
                        self.dirty = true;
                    }
                    Ok(None) => {
                        self.edit = Some(EditState {
                            error: Some("a value is required".to_string()),
                            ..edit
                        });
                    }
                    Err(e) => {
                        self.edit = Some(EditState {
                            error: Some(e),
                            ..edit
                        });
                    }
                }
            }
        }
    }

    fn open_save_dialog(&mut self) {
        let candidates = save_path_candidates(self.loaded_from.as_deref());
        let buffer = candidates
            .first()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "./mqttaudio.json".to_string());
        self.modal = Some(Modal::SaveDialog {
            cursor: buffer.chars().count(),
            buffer,
            candidates,
        });
    }

    fn open_device_picker(&mut self, output: bool, target: Vec<Seg>) {
        let mut items = vec![PickerItem {
            name: None,
            detail: "use the system default device".to_string(),
        }];
        if output {
            let list = crate::audio::engine::output_device_list();
            for d in &list.devices {
                let mut detail = d.category.to_string();
                if let Some(ref caps) = d.native_capabilities {
                    detail = format!(
                        "{} | {} ch, {} Hz, {}",
                        detail,
                        caps.format_channels(),
                        caps.format_rates(),
                        caps.format_formats()
                    );
                } else if let (Some(ch), Some(min), Some(max)) = (
                    d.cpal_channels,
                    d.cpal_sample_rate_min,
                    d.cpal_sample_rate_max,
                ) {
                    if min == max {
                        detail = format!("{} | {} ch, {} Hz", detail, ch, min);
                    } else {
                        detail = format!("{} | {} ch, {}-{} Hz", detail, ch, min, max);
                    }
                }
                if let Some(ref desc) = d.description {
                    detail = format!("{} | {}", desc, detail);
                }
                items.push(PickerItem {
                    name: Some(d.name.clone()),
                    detail,
                });
            }
        } else {
            match crate::audio::input::input_device_list() {
                Ok(list) => {
                    for d in &list {
                        let detail = match d.channels() {
                            Some(ch) => format!("{} ch", ch),
                            None => "capabilities unknown".to_string(),
                        };
                        items.push(PickerItem {
                            name: Some(d.name.clone()),
                            detail,
                        });
                    }
                }
                Err(e) => {
                    self.status = format!("Could not list input devices: {}", e);
                }
            }
        }
        self.modal = Some(Modal::DevicePicker {
            output,
            items,
            cursor: 0,
            target,
        });
    }

    /// Where committing the given row's edit writes its value.
    fn row_target(&self, row: &Row) -> EditTarget {
        if matches!(self.level().kind, LevelKind::Map { .. }) {
            EditTarget::MapValue {
                path: row.path.clone(),
                kind: row.kind,
            }
        } else {
            EditTarget::Field {
                path: row.path.clone(),
                kind: row.kind,
            }
        }
    }

    /// Open a choice picker, pre-selecting the item matching `current`.
    fn open_choice_picker(
        &mut self,
        title: String,
        hint: String,
        edit_label: String,
        items: Vec<Choice>,
        current: Option<String>,
        target: EditTarget,
    ) {
        let cursor = current
            .and_then(|cur| items.iter().position(|c| !c.custom && c.insert_text == cur))
            .unwrap_or(0);
        self.modal = Some(Modal::ChoicePicker {
            title,
            hint,
            edit_label,
            items,
            cursor,
            target,
        });
    }

    /// Channel choices: aliases first (sorted by channel), then unaliased
    /// channel numbers, then free entry.
    fn channel_choices(&self) -> Vec<Choice> {
        let mut aliases: Vec<(String, u64)> = Vec::new();
        if let Some(Value::Object(map)) = self.doc.get(&path_of(&["audio", "channel_aliases"])) {
            for (name, idx) in map {
                if let Some(idx) = idx.as_u64() {
                    aliases.push((name.clone(), idx));
                }
            }
        }
        aliases.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let mut choices: Vec<Choice> = aliases
            .iter()
            .map(|(name, idx)| Choice::new(name, name, &format!("channel {}", idx)))
            .collect();
        let channels = self
            .effective(&path_of(&["audio", "channels"]))
            .0
            .as_u64()
            .or_else(|| aliases.iter().map(|(_, idx)| idx + 1).max())
            .unwrap_or(8);
        for ch in 0..channels {
            if !aliases.iter().any(|(_, idx)| *idx == ch) {
                choices.push(Choice::new(&ch.to_string(), &format!("channel {}", ch), ""));
            }
        }
        choices.push(Choice::custom("other…", "type a channel number or alias"));
        choices
    }

    /// Voice choices: every voice id named in the document, then free entry.
    fn voice_choices(&self) -> Vec<Choice> {
        let mut choices: Vec<Choice> = known_voice_ids(&self.doc)
            .iter()
            .map(|id| Choice::new(id, id, ""))
            .collect();
        choices.push(Choice::custom("other…", "type a new voice name"));
        choices
    }

    /// Prompt for the value at `path`: a picker for choice-like kinds, a text
    /// edit otherwise.
    fn open_value_editor(&mut self, path: Vec<Seg>, kind: FieldKind, label: String) {
        let target = EditTarget::MapValue {
            path: path.clone(),
            kind,
        };
        match kind {
            FieldKind::Enum(options) => {
                self.open_choice_picker(
                    label.clone(),
                    String::new(),
                    label,
                    enum_choices(options),
                    None,
                    target,
                );
            }
            FieldKind::Bool => {
                self.open_choice_picker(
                    label.clone(),
                    String::new(),
                    label,
                    bool_choices(),
                    None,
                    target,
                );
            }
            FieldKind::ChannelRef => {
                let items = self.channel_choices();
                self.open_choice_picker(
                    "Select channel".to_string(),
                    CHANNEL_PICKER_HINT.to_string(),
                    label,
                    items,
                    None,
                    target,
                );
            }
            FieldKind::VoiceRef => {
                let items = self.voice_choices();
                self.open_choice_picker(
                    "Select voice".to_string(),
                    VOICE_PICKER_HINT.to_string(),
                    label,
                    items,
                    None,
                    target,
                );
            }
            _ => {
                self.edit = Some(EditState {
                    label,
                    buffer: String::new(),
                    cursor: 0,
                    target,
                    error: None,
                });
            }
        }
    }

    /// The config the device test should run with: the in-progress document if
    /// it deserializes, otherwise compiled-in defaults.
    fn test_config(&mut self) -> Config {
        match self.doc.to_config() {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Config incomplete, testing with defaults: {}", e);
                Config::default()
            }
        }
    }

    fn start_output_test_screen(
        &mut self,
        device_name: Option<String>,
        return_to_picker: Option<(bool, Vec<Seg>)>,
    ) {
        let config = self.test_config();
        let aliases_map = config.audio.channel_aliases.clone();
        let (session, error) = match start_output_test(&config, device_name.as_deref()) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        let channels = session.as_ref().map(|s| s.channels).unwrap_or(0);
        let mut aliases: Vec<Option<String>> = vec![None; channels];
        for (name, idx) in &aliases_map {
            if *idx < channels {
                aliases[*idx] = Some(name.clone());
            }
        }
        self.modal = Some(Modal::OutputTest(OutputTestState {
            session,
            error,
            cursor: 0,
            sweep: None,
            device_label: device_name.unwrap_or_else(|| "(default)".to_string()),
            aliases,
            return_to_picker,
        }));
    }

    fn start_input_test_screen(
        &mut self,
        device_name: Option<String>,
        target: &[Seg],
        return_to_picker: Option<(bool, Vec<Seg>)>,
    ) {
        let config = self.test_config();
        // Use the latency configured on the input entry being edited (the
        // picker's target path is .../device; latency_ms is its sibling).
        let latency_ms = if target.len() >= 2 {
            let mut sibling = target[..target.len() - 1].to_vec();
            sibling.push(Seg::Key("latency_ms".to_string()));
            self.doc
                .get(&sibling)
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .unwrap_or(20)
        } else {
            20
        };
        let (session, error) =
            match start_input_test(device_name.as_deref(), latency_ms, config.audio.sample_rate) {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e)),
            };
        self.modal = Some(Modal::InputTest(InputTestState {
            session,
            error,
            device_label: device_name.unwrap_or_else(|| "(default)".to_string()),
            return_to_picker,
        }));
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let modal = self.modal.take();
        match modal {
            Some(Modal::DevicePicker {
                output,
                items,
                mut cursor,
                target,
            }) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {}
                KeyCode::Up => {
                    cursor = cursor.saturating_sub(1);
                    self.modal = Some(Modal::DevicePicker {
                        output,
                        items,
                        cursor,
                        target,
                    });
                }
                KeyCode::Down => {
                    if cursor + 1 < items.len() {
                        cursor += 1;
                    }
                    self.modal = Some(Modal::DevicePicker {
                        output,
                        items,
                        cursor,
                        target,
                    });
                }
                KeyCode::Enter => {
                    match &items[cursor].name {
                        Some(name) => {
                            self.doc.set(&target, Value::String(name.clone()));
                        }
                        None => self.doc.unset(&target),
                    }
                    self.dirty = true;
                    self.status = format!(
                        "Device set to {}",
                        items[cursor].name.as_deref().unwrap_or("(default)")
                    );
                }
                KeyCode::Char('t') => {
                    let name = items[cursor].name.clone();
                    let back = Some((output, target.clone()));
                    if output {
                        self.start_output_test_screen(name, back);
                    } else {
                        self.start_input_test_screen(name, &target, back);
                    }
                }
                _ => {
                    self.modal = Some(Modal::DevicePicker {
                        output,
                        items,
                        cursor,
                        target,
                    });
                }
            },
            Some(Modal::ChoicePicker {
                title,
                hint,
                edit_label,
                items,
                mut cursor,
                target,
            }) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {}
                KeyCode::Up | KeyCode::Down => {
                    if key.code == KeyCode::Up {
                        cursor = cursor.saturating_sub(1);
                    } else if cursor + 1 < items.len() {
                        cursor += 1;
                    }
                    self.modal = Some(Modal::ChoicePicker {
                        title,
                        hint,
                        edit_label,
                        items,
                        cursor,
                        target,
                    });
                }
                KeyCode::Enter => {
                    let Some(item) = items.get(cursor) else {
                        return;
                    };
                    if item.custom {
                        // Fall back to the normal free-text edit.
                        self.edit = Some(EditState {
                            label: edit_label,
                            buffer: String::new(),
                            cursor: 0,
                            target,
                            error: None,
                        });
                    } else if let EditTarget::MapKey { base, value_kind } = target {
                        // A picked key chains into a prompt for its value,
                        // typed by the key when the picker says so.
                        let key_name = item.insert_text.clone();
                        let mut path = base;
                        path.push(Seg::Key(key_name.clone()));
                        if self.doc.get(&path).is_some() {
                            self.status = format!("'{}' already exists", key_name);
                        } else {
                            let kind = item.value_kind.unwrap_or(value_kind);
                            self.open_value_editor(path, kind, format!("value for {}", key_name));
                        }
                    } else {
                        // Commit through the edit pipeline so parsing,
                        // dirty-tracking, and cursor placement all apply.
                        self.edit = Some(EditState {
                            label: edit_label,
                            cursor: item.insert_text.chars().count(),
                            buffer: item.insert_text.clone(),
                            target,
                            error: None,
                        });
                        self.commit_edit();
                    }
                }
                _ => {
                    self.modal = Some(Modal::ChoicePicker {
                        title,
                        hint,
                        edit_label,
                        items,
                        cursor,
                        target,
                    });
                }
            },
            Some(Modal::OutputTest(mut state)) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    drop(state.session.take());
                    if let Some((output, target)) = state.return_to_picker {
                        self.open_device_picker(output, target);
                    }
                }
                KeyCode::Up => {
                    state.cursor = state.cursor.saturating_sub(1);
                    self.modal = Some(Modal::OutputTest(state));
                }
                KeyCode::Down => {
                    let channels = state.session.as_ref().map(|s| s.channels).unwrap_or(0);
                    if state.cursor + 1 < channels {
                        state.cursor += 1;
                    }
                    self.modal = Some(Modal::OutputTest(state));
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let cursor = state.cursor;
                    if let Some(session) = state.session.as_mut() {
                        if let Err(e) = session.play_tone_on_channel(cursor, TEST_TONE_HZ) {
                            state.error = Some(e);
                        }
                    }
                    self.modal = Some(Modal::OutputTest(state));
                }
                KeyCode::Char('b') => {
                    let cursor = state.cursor;
                    if let Some(session) = state.session.as_mut() {
                        if let Err(e) = session.play_tone_on_channel(cursor, BASS_TONE_HZ) {
                            state.error = Some(e);
                        }
                    }
                    self.modal = Some(Modal::OutputTest(state));
                }
                KeyCode::Char('a') => {
                    state.sweep = Some((0, Instant::now() - sweep_interval()));
                    self.modal = Some(Modal::OutputTest(state));
                }
                _ => self.modal = Some(Modal::OutputTest(state)),
            },
            Some(Modal::InputTest(mut state)) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    drop(state.session.take());
                    if let Some((output, target)) = state.return_to_picker {
                        self.open_device_picker(output, target);
                    }
                }
                _ => self.modal = Some(Modal::InputTest(state)),
            },
            Some(Modal::SaveDialog {
                mut buffer,
                mut cursor,
                candidates,
            }) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let path = PathBuf::from(buffer.trim());
                    match save_document(&self.doc, &path) {
                        Ok(outcome) => {
                            self.dirty = false;
                            self.loaded_from = Some(path.clone());
                            self.status = match outcome.backup {
                                Some(bak) => format!(
                                    "Saved {} (backup at {}) — restart mqttaudio to apply",
                                    path.display(),
                                    bak.display()
                                ),
                                None => {
                                    format!("Saved {} — restart mqttaudio to apply", path.display())
                                }
                            };
                        }
                        Err(errors) => {
                            self.modal = Some(Modal::Errors {
                                title: "Cannot save: configuration is invalid".to_string(),
                                errors,
                            });
                        }
                    }
                }
                KeyCode::Up | KeyCode::Down => {
                    let current = candidates
                        .iter()
                        .position(|c| c.display().to_string() == buffer);
                    let next = match (key.code, current) {
                        (KeyCode::Down, Some(i)) => (i + 1) % candidates.len(),
                        (KeyCode::Up, Some(i)) => (i + candidates.len() - 1) % candidates.len(),
                        _ => 0,
                    };
                    if let Some(c) = candidates.get(next) {
                        buffer = c.display().to_string();
                        cursor = buffer.chars().count();
                    }
                    self.modal = Some(Modal::SaveDialog {
                        buffer,
                        cursor,
                        candidates,
                    });
                }
                KeyCode::Char(c) => {
                    let byte = byte_index(&buffer, cursor);
                    buffer.insert(byte, c);
                    cursor += 1;
                    self.modal = Some(Modal::SaveDialog {
                        buffer,
                        cursor,
                        candidates,
                    });
                }
                KeyCode::Backspace => {
                    if cursor > 0 {
                        cursor -= 1;
                        let byte = byte_index(&buffer, cursor);
                        buffer.remove(byte);
                    }
                    self.modal = Some(Modal::SaveDialog {
                        buffer,
                        cursor,
                        candidates,
                    });
                }
                KeyCode::Left => {
                    cursor = cursor.saturating_sub(1);
                    self.modal = Some(Modal::SaveDialog {
                        buffer,
                        cursor,
                        candidates,
                    });
                }
                KeyCode::Right => {
                    if cursor < buffer.chars().count() {
                        cursor += 1;
                    }
                    self.modal = Some(Modal::SaveDialog {
                        buffer,
                        cursor,
                        candidates,
                    });
                }
                _ => {
                    self.modal = Some(Modal::SaveDialog {
                        buffer,
                        cursor,
                        candidates,
                    });
                }
            },
            Some(Modal::Errors { .. }) => {
                // Any key dismisses.
            }
            Some(Modal::ConfirmQuit) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.quit = true,
                _ => {}
            },
            Some(Modal::LoadFailed { path, error }) => match key.code {
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.doc = ConfigDocument::new();
                    self.status = format!(
                        "Started from defaults; {} is untouched until you save",
                        path.display()
                    );
                }
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                _ => self.modal = Some(Modal::LoadFailed { path, error }),
            },
            None => {}
        }
    }

    /// Periodic work: drive device-test sessions and the channel sweep.
    pub fn tick(&mut self) {
        match self.modal.as_mut() {
            Some(Modal::OutputTest(state)) => {
                if let Some(session) = state.session.as_mut() {
                    if let Err(e) = session.poll() {
                        state.error = Some(e);
                    }
                    if let Some((next, last)) = state.sweep {
                        if last.elapsed() >= sweep_interval() {
                            if next < session.channels {
                                state.cursor = next;
                                if let Err(e) = session.play_tone_on_channel(next, TEST_TONE_HZ) {
                                    state.error = Some(e);
                                }
                                state.sweep = Some((next + 1, Instant::now()));
                            } else {
                                state.sweep = None;
                            }
                        }
                    }
                }
            }
            Some(Modal::InputTest(state)) => {
                if let Some(session) = state.session.as_mut() {
                    session.poll();
                }
            }
            _ => {}
        }
    }
}

/// Gap between sweep tones: the tone itself plus a beat of silence.
fn sweep_interval() -> std::time::Duration {
    std::time::Duration::from_millis(TONE_DURATION_MS as u64 + 300)
}

/// Byte offset of character `cursor` in `s` (cursor positions are in chars).
fn byte_index(s: &str, cursor: usize) -> usize {
    s.char_indices()
        .nth(cursor)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Context line shown under channel pickers.
const CHANNEL_PICKER_HINT: &str =
    "Aliases are defined in Audio > channel_aliases; numbers are 0-indexed.";

/// Context line shown under voice pickers.
const VOICE_PICKER_HINT: &str =
    "Voices are created at runtime by Play commands; any name is valid.";

/// Every voice id named in the document: input voice_ids (an input without
/// one plays under the default "mic") and the voices in ducking rules.
pub fn known_voice_ids(doc: &ConfigDocument) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    if let Some(Value::Array(inputs)) = doc.get(&path_of(&["inputs"])) {
        for input in inputs {
            match input.get("voice_id").and_then(|v| v.as_str()) {
                Some(v) if !v.is_empty() => ids.push(v.to_string()),
                Some(_) => {}
                None => ids.push("mic".to_string()),
            }
        }
    }
    if let Some(Value::Array(rules)) = doc.get(&path_of(&["ducking_rules"])) {
        for rule in rules {
            if let Some(p) = rule.get("primary_voice").and_then(|v| v.as_str()) {
                if !p.is_empty() {
                    ids.push(p.to_string());
                }
            }
            if let Some(Value::Array(ducked)) = rule.get("ducked_voices") {
                for d in ducked {
                    if let Some(s) = d.as_str() {
                        if !s.is_empty() {
                            ids.push(s.to_string());
                        }
                    }
                }
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Picker choices for an enum's options.
fn enum_choices(options: &'static [&'static str]) -> Vec<Choice> {
    options.iter().map(|o| Choice::new(o, o, "")).collect()
}

/// Picker choices for a boolean value.
fn bool_choices() -> Vec<Choice> {
    vec![
        Choice::new("true", "true", ""),
        Choice::new("false", "false", ""),
    ]
}

/// Picker choices for adding a macro parameter: the known command parameters
/// not already present, then a custom-key entry.
fn macro_param_choices(existing: &Value) -> Vec<Choice> {
    let mut choices: Vec<Choice> = super::fields::MACRO_PARAM_FIELDS
        .iter()
        .filter(|spec| existing.get(spec.key).is_none())
        .map(|spec| {
            let mut c = Choice::new(spec.key, spec.label, spec.help);
            c.value_kind = Some(spec.kind);
            c
        })
        .collect();
    choices.push(Choice::custom(
        "other…",
        "type a parameter name; the value is raw JSON",
    ));
    choices
}

/// Help text for one element of a scalar list.
fn scalar_item_help(elem: &FieldKind) -> &'static str {
    match elem {
        FieldKind::ChannelRef => {
            "A channel number (0-indexed) or an alias defined in audio.channel_aliases."
        }
        FieldKind::VoiceRef => {
            "A voice name, as used by a Play command's \"voice\" parameter or an input's voice_id."
        }
        _ => "One entry in this list.",
    }
}

/// Help text for one entry of a map, by its value kind.
fn map_item_help(value_kind: &FieldKind) -> &'static str {
    match value_kind {
        FieldKind::UInt { .. } => "The channel number (0-indexed) this alias refers to.",
        FieldKind::Float { .. } => "Calibration gain for this channel (0.0 - 1.0).",
        FieldKind::MapToJson => {
            "A macro: command parameters merged into commands that reference it by name."
        }
        _ => "One entry in this map.",
    }
}
