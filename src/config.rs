// SPDX-License-Identifier: GPL-3.0-or-later
//! The user's settings: what `init.rhai` gave `forge.plugin.config("meow", { ... })`,
//! turned into [`Options`]. The names are Meow's own, so a setting here is
//! the same one the Emacs documentation talks about.
//!
//! A key that isn't known, or has the wrong kind of value, is an error that
//! names it, so a typo isn't silent.

use serde_json::Value;
use std::cell::RefCell;

/// Every setting, for the error that lists them.
const KNOWN: &[&str] = &[
    "meow-use-cursor-position-hack",
    "meow-use-enhanced-selection-effect",
    "meow-expand-exclude-mode-list",
    "meow-keypad-execute-on-beacons",
    "meow-selection-command-fallback",
    "meow-replace-state-name-list",
    "meow-select-on-change",
    "meow-select-on-append",
    "meow-select-on-insert",
    "meow-select-on-open",
    "meow-expand-hint-remove-delay",
    "meow-next-thing-include-syntax",
    "meow-expand-hint-counts",
    "meow-keypad-message",
    "meow-char-thing-table",
    "meow-thing-selection-directions",
    "meow-display-thing-help",
    "meow-pop-or-unpop-to-mark-repeat-unpop",
    "meow-keypad-describe-delay",
    "meow-grab-fill-commands",
    "meow-visit-collect-min-length",
    "meow-visit-sanitize-completion",
    "meow-use-clipboard",
    "meow-webkit-keys",
    "meow-use-keypad-when-execute-kbd",
    "meow-use-dynamic-face-color",
    "meow-mode-state-list",
    "meow-update-display-in-macro",
    "meow-expand-selection-type",
    "meow-keypad-leader-transparent",
    "meow-keypad-leader-dispatch",
    "meow-keypad-meta-prefix",
    "meow-keypad-ctrl-meta-prefix",
    "meow-keypad-literal-prefix",
    "meow-keypad-start-keys",
    "meow-goto-line-function",
    "meow-keypad-message-prefix",
    "meow-replace-pop-command-start-indexes",
];

fn bad(key: &str, wants: &str) -> String {
    format!("meow: {key} must be {wants}")
}

fn flag(key: &str, v: &Value) -> Result<bool, String> {
    v.as_bool().ok_or_else(|| bad(key, "true or false"))
}

fn number(key: &str, v: &Value) -> Result<f64, String> {
    v.as_f64().ok_or_else(|| bad(key, "a number"))
}

fn count(key: &str, v: &Value) -> Result<u32, String> {
    v.as_u64()
        .filter(|n| *n <= u32::MAX as u64)
        .map(|n| n as u32)
        .ok_or_else(|| bad(key, "a whole number"))
}

fn text(key: &str, v: &Value) -> Result<String, String> {
    v.as_str().map(String::from).ok_or_else(|| bad(key, "text"))
}

fn words(key: &str, v: &Value) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or_else(|| bad(key, "a list of text"))?
        .iter()
        .map(|w| text(key, w))
        .collect()
}

/// One character of a setting.
fn chr(key: &str, v: &Value) -> Result<char, String> {
    v.as_str()
        .and_then(|s| {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        })
        .filter(|c| s_is_one(*c))
        .ok_or_else(|| bad(key, "a single character"))
}

fn s_is_one(c: char) -> bool {
    c.is_ascii_graphic() || c == ' '
}

/// A list of pairs: a table of `{"name": value}`.
fn pairs(key: &str, v: &Value) -> Result<Vec<(String, Value)>, String> {
    let map = v.as_object().ok_or_else(|| bad(key, "a table"))?;
    Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// The settings, over Meow's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    // ---------------------------------------------------------- display
    /// Whether to use the cursor position hack.
    pub use_cursor_position_hack: bool,
    /// Whether to use the enhanced cursor effect.
    pub use_enhanced_selection_effect: bool,
    /// Whether the position hint's color is worked out from the theme.
    pub use_dynamic_face_color: bool,
    /// Whether the mode line is updated while a keyboard macro runs.
    pub update_display_in_macro: String,
    /// The names the states are shown under in the indicator.
    pub replace_state_name_list: Vec<(String, String)>,

    // -------------------------------------------------------- expansion
    /// The major modes where `meow-expand` does nothing.
    pub expand_exclude_mode_list: Vec<String>,
    /// The delay before the position hint goes away.
    pub expand_hint_remove_delay: f64,
    /// The largest number each kind of expand hint may show.
    pub expand_hint_counts: Vec<(String, u32)>,
    /// Whether the expand commands make a selection or expand it.
    pub expand_selection_type: String,
    /// The syntax classes the next-thing commands include.
    pub next_thing_include_syntax: Vec<(String, String, String)>,

    // ------------------------------------------------------------ keypad
    /// Whether the keypad says what it is doing.
    pub keypad_message: bool,
    /// What the keypad's messages start with.
    pub keypad_message_prefix: String,
    /// The key that stands for M- in the keypad.
    pub keypad_meta_prefix: char,
    /// The key that stands for C-M- in the keypad.
    pub keypad_ctrl_meta_prefix: char,
    /// The key that stands for no modifier at all in the keypad.
    pub keypad_literal_prefix: char,
    /// The keys that start a C- sequence, and the letter they carry.
    pub keypad_start_keys: Vec<(char, char)>,
    /// How long the keypad waits before describing a key.
    pub keypad_describe_delay: f64,
    /// Whether a beacon's command is run from the keypad directly.
    pub keypad_execute_on_beacons: bool,
    /// Whether the keypad is used when a key sequence is executed.
    pub use_keypad_when_execute_kbd: bool,
    /// What the keypad does with a key it doesn't know.
    pub keypad_leader_transparent: String,
    /// Where the keypad looks when a key isn't bound.
    pub keypad_leader_dispatch: Option<String>,

    // ------------------------------------------------------------- things
    /// The keys that name the things, and what they name.
    pub char_thing_table: Vec<(char, String)>,
    /// Whether the user gave a thing table of their own, which is then
    /// used instead of the one the layout has.
    pub char_thing_table_is_set: bool,
    /// Which way each thing command selects.
    pub thing_selection_directions: Vec<(String, String)>,
    /// Whether the thing prompt says what the things are.
    pub display_thing_help: bool,

    // --------------------------------------------------------- selection
    /// Whether leaving insert state after a change selects what was typed.
    pub select_on_change: bool,
    /// Whether leaving insert state after an append selects what was typed.
    pub select_on_append: bool,
    /// Whether leaving insert state after an insert selects what was typed.
    pub select_on_insert: bool,
    /// Whether leaving insert state after opening a line selects it.
    pub select_on_open: bool,
    /// What a selection command does when there is no selection.
    pub selection_command_fallback: Vec<(String, String)>,
    /// Whether a killed text also goes to the editor's own kill ring.
    pub use_clipboard: bool,
    /// Vim-style keys in web pages (webkit.rs).
    pub webkit_keys: bool,
    /// Whether a repeated mark jump unpops again.
    pub pop_or_unpop_to_mark_repeat_unpop: bool,
    /// Where each replacing command starts in the kill ring.
    pub replace_pop_command_start_indexes: Vec<(String, u32)>,

    // ------------------------------------------------------------ states
    /// The state each major mode starts in.
    pub mode_state_list: Vec<(String, String)>,
    /// The commands meow fills in with the grabbed text.
    pub grab_fill_commands: Vec<String>,
    /// The command `meow-goto-line` runs.
    pub goto_line_function: Option<String>,
    /// The shortest symbol `meow-visit` collects.
    pub visit_collect_min_length: u32,
    /// Whether `meow-visit` shows its symbols sanitized.
    pub visit_sanitize_completion: bool,
}

impl Default for Options {
    /// Meow's own defaults, from `meow-var.el`.
    fn default() -> Options {
        Options {
            use_cursor_position_hack: false,
            use_enhanced_selection_effect: false,
            use_dynamic_face_color: true,
            update_display_in_macro: "except-last-macro".into(),
            replace_state_name_list: vec![
                ("normal".into(), "NORMAL".into()),
                ("motion".into(), "MOTION".into()),
                ("keypad".into(), "KEYPAD".into()),
                ("insert".into(), "INSERT".into()),
                ("beacon".into(), "BEACON".into()),
            ],
            expand_exclude_mode_list: vec!["markdown-mode".into(), "org-mode".into()],
            expand_hint_remove_delay: 1.0,
            expand_hint_counts: vec![
                ("word".into(), 30),
                ("line".into(), 30),
                ("block".into(), 30),
                ("find".into(), 30),
                ("till".into(), 30),
                ("symbol".into(), 30),
            ],
            expand_selection_type: "select".into(),
            next_thing_include_syntax: vec![
                ("word".into(), " _w".into(), " _w".into()),
                ("symbol".into(), " _w".into(), " _w".into()),
            ],
            keypad_message: true,
            keypad_message_prefix: "Keypad: ".into(),
            keypad_meta_prefix: 'm',
            keypad_ctrl_meta_prefix: 'g',
            keypad_literal_prefix: ' ',
            keypad_start_keys: vec![('c', 'c'), ('h', 'h'), ('x', 'x')],
            keypad_describe_delay: 0.5,
            keypad_execute_on_beacons: false,
            use_keypad_when_execute_kbd: true,
            keypad_leader_transparent: "motion".into(),
            keypad_leader_dispatch: None,
            char_thing_table: vec![
                ('r', "round".into()),
                ('s', "square".into()),
                ('c', "curly".into()),
                ('g', "string".into()),
                ('e', "symbol".into()),
                ('w', "window".into()),
                ('b', "buffer".into()),
                ('p', "paragraph".into()),
                ('l', "line".into()),
                ('v', "visual-line".into()),
                ('d', "defun".into()),
                ('.', "sentence".into()),
            ],
            char_thing_table_is_set: false,
            thing_selection_directions: vec![
                ("inner".into(), "forward".into()),
                ("bounds".into(), "backward".into()),
                ("beginning".into(), "backward".into()),
                ("end".into(), "forward".into()),
            ],
            display_thing_help: true,
            select_on_change: true,
            select_on_append: false,
            select_on_insert: false,
            select_on_open: false,
            selection_command_fallback: vec![
                ("meow-change".into(), "meow-change-char".into()),
                ("meow-kill".into(), "meow-C-k".into()),
                ("meow-save".into(), "kill-ring-save".into()),
                ("meow-cancel-selection".into(), "keyboard-quit".into()),
                ("meow-pop-selection".into(), "meow-pop-grab".into()),
                ("meow-beacon-change".into(), "meow-beacon-change-char".into()),
                ("meow-expand".into(), "meow-digit-argument".into()),
            ],
            use_clipboard: false,
            webkit_keys: true,
            pop_or_unpop_to_mark_repeat_unpop: false,
            replace_pop_command_start_indexes: vec![
                ("meow-replace".into(), 1),
                ("meow-replace-char".into(), 1),
                ("meow-replace-save".into(), 2),
            ],
            mode_state_list: vec![
                ("conf-mode".into(), "normal".into()),
                ("fundamental-mode".into(), "normal".into()),
                ("help-mode".into(), "motion".into()),
                ("prog-mode".into(), "normal".into()),
                ("special-mode".into(), "motion".into()),
                ("text-mode".into(), "normal".into()),
            ],
            grab_fill_commands: vec!["meow-query-replace".into(), "meow-query-replace-regexp".into()],
            goto_line_function: None,
            visit_collect_min_length: 1,
            visit_sanitize_completion: true,
        }
    }
}

impl Options {
    /// The value of a setting that is a table of names and values.
    fn lookup<'a>(&'a self, table: &'a [(String, String)], name: &str) -> Option<&'a str> {
        table
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// What a selection command does when there is no selection.
    pub fn fallback(&self, command: &str) -> Option<&str> {
        self.lookup(&self.selection_command_fallback, command)
    }

    /// Which way a thing command selects.
    pub fn thing_direction(&self, command: &str) -> &str {
        self.lookup(&self.thing_selection_directions, command)
            .unwrap_or("forward")
    }

    /// The name a state is shown under.
    pub fn state_name<'a>(&'a self, state: &'a str) -> &'a str {
        self.lookup(&self.replace_state_name_list, state).unwrap_or(state)
    }

    /// The state the user gave this exact major mode, if any.
    pub fn configured_state(&self, mode: &str) -> Option<&str> {
        self.lookup(&self.mode_state_list, mode)
    }

    /// The letter a keypad start key carries.
    pub fn keypad_start(&self, key: char) -> Option<char> {
        self.keypad_start_keys
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
    }

    /// Where a replacing command starts in the kill ring.
    pub fn replace_pop_index(&self, command: &str) -> Option<u32> {
        self.replace_pop_command_start_indexes
            .iter()
            .find(|(k, _)| k == command)
            .map(|(_, v)| *v)
    }

    /// The largest number an expand hint of this kind may show.
    pub fn expand_hint_count(&self, kind: &str) -> u32 {
        self.expand_hint_counts
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, v)| *v)
            .unwrap_or(30)
    }

    /// The syntax classes a next-thing command includes, forward and back.
    pub fn thing_syntax(&self, thing: &str) -> Option<(&str, &str)> {
        self.next_thing_include_syntax
            .iter()
            .find(|(t, _, _)| t == thing)
            .map(|(_, forward, back)| (forward.as_str(), back.as_str()))
    }

    /// Whether the keypad lets a key it doesn't know through to the state
    /// underneath it.
    pub fn keypad_is_transparent(&self, state: &str) -> bool {
        match self.keypad_leader_transparent.as_str() {
            "always" => true,
            "motion" => state == "motion",
            "normal" => state == "normal",
            _ => false,
        }
    }
}

thread_local! {
    /// The settings the plugin is running with.
    static OPTIONS: RefCell<Options> = RefCell::new(Options::default());
}

/// The settings the plugin is running with.
pub fn with<R>(f: impl FnOnce(&Options) -> R) -> R {
    OPTIONS.with(|o| f(&o.borrow()))
}

/// The settings, read from the editor's configuration. A setting that is
/// wrong is reported in the echo area, and the defaults are kept.
pub fn load() {
    let json = forge_plugin_sdk::config_json();
    if json.trim().is_empty() {
        return;
    }
    match parse(&json) {
        Ok(o) => OPTIONS.with(|slot| *slot.borrow_mut() = o),
        Err(e) => forge_plugin_sdk::message(&e),
    }
}

/// The settings in `json` (an object), over the defaults.
pub fn parse(json: &str) -> Result<Options, String> {
    let mut o = Options::default();
    let v: Value = serde_json::from_str(json).map_err(|e| format!("meow: settings: {e}"))?;
    let Some(map) = v.as_object() else {
        return Err("meow: settings must be a table".into());
    };
    for (key, v) in map {
        match key.as_str() {
            "meow-use-cursor-position-hack" => o.use_cursor_position_hack = flag(key, v)?,
            "meow-use-enhanced-selection-effect" => o.use_enhanced_selection_effect = flag(key, v)?,
            "meow-use-dynamic-face-color" => o.use_dynamic_face_color = flag(key, v)?,
            "meow-update-display-in-macro" => {
                let s = text(key, v)?;
                if !matches!(s.as_str(), "except-last-macro" | "always" | "never") {
                    return Err(bad(key, "\"except-last-macro\", \"always\" or \"never\""));
                }
                o.update_display_in_macro = s;
            }
            "meow-replace-state-name-list" => {
                let mut list = Vec::new();
                for (state, name) in pairs(key, v)? {
                    list.push((state, text(key, &name)?));
                }
                o.replace_state_name_list = list;
            }
            "meow-expand-exclude-mode-list" => o.expand_exclude_mode_list = words(key, v)?,
            "meow-expand-hint-remove-delay" => o.expand_hint_remove_delay = number(key, v)?,
            "meow-expand-hint-counts" => {
                let mut list = Vec::new();
                for (kind, n) in pairs(key, v)? {
                    list.push((kind, count(key, &n)?));
                }
                o.expand_hint_counts = list;
            }
            "meow-expand-selection-type" => {
                let s = text(key, v)?;
                if !matches!(s.as_str(), "select" | "expand") {
                    return Err(bad(key, "\"select\" or \"expand\""));
                }
                o.expand_selection_type = s;
            }
            "meow-next-thing-include-syntax" => {
                let list = v
                    .as_array()
                    .ok_or_else(|| bad(key, "a list of {thing, forward, backward}"))?;
                let mut out = Vec::new();
                for item in list {
                    let item = item
                        .as_array()
                        .ok_or_else(|| bad(key, "a list of {thing, forward, backward}"))?;
                    let [thing, forward, backward] = item.as_slice() else {
                        return Err(bad(key, "a list of {thing, forward, backward}"));
                    };
                    out.push((
                        text(key, thing)?,
                        text(key, forward)?,
                        text(key, backward)?,
                    ));
                }
                o.next_thing_include_syntax = out;
            }
            "meow-keypad-message" => o.keypad_message = flag(key, v)?,
            "meow-keypad-message-prefix" => o.keypad_message_prefix = text(key, v)?,
            "meow-keypad-meta-prefix" => o.keypad_meta_prefix = chr(key, v)?,
            "meow-keypad-ctrl-meta-prefix" => o.keypad_ctrl_meta_prefix = chr(key, v)?,
            "meow-keypad-literal-prefix" => o.keypad_literal_prefix = chr(key, v)?,
            "meow-keypad-start-keys" => {
                let list = v
                    .as_array()
                    .ok_or_else(|| bad(key, "a list of {from, to}"))?;
                let mut out = Vec::new();
                for item in list {
                    let item = item
                        .as_array()
                        .ok_or_else(|| bad(key, "a list of {from, to}"))?;
                    let [from, to] = item.as_slice() else {
                        return Err(bad(key, "a list of {from, to}"));
                    };
                    out.push((chr(key, from)?, chr(key, to)?));
                }
                o.keypad_start_keys = out;
            }            "meow-keypad-describe-delay" => o.keypad_describe_delay = number(key, v)?,
            "meow-keypad-execute-on-beacons" => o.keypad_execute_on_beacons = flag(key, v)?,
            "meow-use-keypad-when-execute-kbd" => o.use_keypad_when_execute_kbd = flag(key, v)?,
            "meow-keypad-leader-transparent" => {
                let s = text(key, v)?;
                if !matches!(s.as_str(), "always" | "motion" | "normal" | "never") {
                    return Err(bad(key, "\"always\", \"motion\", \"normal\" or \"never\""));
                }
                o.keypad_leader_transparent = s;
            }
            "meow-keypad-leader-dispatch" => {
                o.keypad_leader_dispatch = match v {
                    Value::Null => None,
                    other => Some(text(key, other)?),
                }
            }
            "meow-char-thing-table" => {
                let list = v
                    .as_array()
                    .ok_or_else(|| bad(key, "a list of {key, thing}"))?;
                let mut out = Vec::new();
                for item in list {
                    let item = item
                        .as_array()
                        .ok_or_else(|| bad(key, "a list of {key, thing}"))?;
                    let [k, thing] = item.as_slice() else {
                        return Err(bad(key, "a list of {key, thing}"));
                    };
                    out.push((chr(key, k)?, text(key, thing)?));
                }
                o.char_thing_table = out;
                o.char_thing_table_is_set = true;
            }
            "meow-thing-selection-directions" => {
                let mut list = Vec::new();
                for (command, direction) in pairs(key, v)? {
                    list.push((command, text(key, &direction)?));
                }
                o.thing_selection_directions = list;
            }
            "meow-display-thing-help" => o.display_thing_help = flag(key, v)?,
            "meow-select-on-change" => o.select_on_change = flag(key, v)?,
            "meow-select-on-append" => o.select_on_append = flag(key, v)?,
            "meow-select-on-insert" => o.select_on_insert = flag(key, v)?,
            "meow-select-on-open" => o.select_on_open = flag(key, v)?,
            "meow-selection-command-fallback" => {
                let mut list = Vec::new();
                for (command, fallback) in pairs(key, v)? {
                    list.push((command, text(key, &fallback)?));
                }
                o.selection_command_fallback = list;
            }
            "meow-use-clipboard" => o.use_clipboard = flag(key, v)?,
            "meow-webkit-keys" => o.webkit_keys = flag(key, v)?,
            "meow-pop-or-unpop-to-mark-repeat-unpop" => {
                o.pop_or_unpop_to_mark_repeat_unpop = flag(key, v)?
            }
            "meow-replace-pop-command-start-indexes" => {
                let mut list = Vec::new();
                for (command, index) in pairs(key, v)? {
                    list.push((command, count(key, &index)?));
                }
                o.replace_pop_command_start_indexes = list;
            }
            "meow-mode-state-list" => {
                let mut list = Vec::new();
                for (mode, state) in pairs(key, v)? {
                    list.push((mode, text(key, &state)?));
                }
                o.mode_state_list = list;
            }
            "meow-grab-fill-commands" => o.grab_fill_commands = words(key, v)?,
            "meow-goto-line-function" => {
                o.goto_line_function = match v {
                    Value::Null => None,
                    other => Some(text(key, other)?),
                }
            }
            "meow-visit-collect-min-length" => o.visit_collect_min_length = count(key, v)?,
            "meow-visit-sanitize-completion" => o.visit_sanitize_completion = flag(key, v)?,
            other => {
                return Err(format!(
                    "meow: unknown setting {other:?} (known: {})",
                    KNOWN.join(", ")
                ));
            }
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_given_is_the_defaults() {
        assert_eq!(parse("{}").unwrap(), Options::default());
        let o = Options::default();
        assert_eq!(o.configured_state("text-mode"), Some("normal"));
        assert_eq!(o.configured_state("help-mode"), Some("motion"));
        assert_eq!(o.configured_state("special-mode"), Some("motion"));
        assert_eq!(o.configured_state("webkit-mode"), None);
    }

    #[test]
    fn settings_change_what_the_plugin_does() {
        let o = parse(
            r#"{"meow-keypad-message":false,"meow-keypad-meta-prefix":"M",
                "meow-select-on-insert":true,"meow-char-thing-table":[["z","round"]]}"#,
        )
        .unwrap();
        assert!(!o.keypad_message);
        assert_eq!(o.keypad_meta_prefix, 'M');
        assert!(o.select_on_insert);
        assert_eq!(o.char_thing_table, vec![('z', "round".to_string())]);
    }

    #[test]
    fn the_lookups_use_the_settings() {
        let o = parse(
            r#"{"meow-selection-command-fallback":{"meow-kill":"meow-delete"},
                "meow-mode-state-list":{"text-mode":"motion"},
                "meow-keypad-start-keys":[["z","y"]],
                "meow-replace-pop-command-start-indexes":{"meow-replace":3}}"#,
        )
        .unwrap();
        assert_eq!(o.fallback("meow-kill"), Some("meow-delete"));
        assert_eq!(o.fallback("meow-nothing"), None);
        assert_eq!(o.configured_state("text-mode"), Some("motion"));
        assert_eq!(o.configured_state("rust-mode"), None);
        assert_eq!(o.keypad_start('z'), Some('y'));
        assert_eq!(o.keypad_start('x'), None);
        assert_eq!(o.replace_pop_index("meow-replace"), Some(3));
    }

    #[test]
    fn mistakes_name_the_setting() {
        let e = |j: &str| parse(j).unwrap_err();
        assert!(e(r#"{"meow-stars":true}"#).contains("unknown setting \"meow-stars\""));
        assert!(e(r#"{"meow-keypad-message":"yes"}"#).contains("must be true or false"));
        assert!(e(r#"{"meow-keypad-meta-prefix":"ab"}"#).contains("single character"));
        assert!(e(r#"{"meow-expand-selection-type":"nope"}"#).contains("\"select\" or \"expand\""));
        assert!(e(r#"{"meow-char-thing-table":{"a":"b"}}"#).contains("a list of {key, thing}"));
        assert!(e("[1]").contains("must be a table"));
        assert!(e("nope").contains("settings"));
    }
}
