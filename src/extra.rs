// SPDX-License-Identifier: GPL-3.0-or-later
//! The rest of Meow's command surface that a plugin can reach: editing
//! shortcuts, the beacon (done here as "do this at every marked place" —
//! the editor has no multiple cursors), extra states and key definition,
//! thing help, keypad undo and display setup.

use super::*;
use forge_plugin_sdk::{decorations, Decoration};
use std::collections::BTreeMap;

/// One place the beacon will act on, as offsets into the buffer.
pub type Mark = (usize, usize);

thread_local! {
    /// (buffer id, the places) while the beacon is on.
    static BEACON: RefCell<Option<(u32, Vec<Mark>)>> = const { RefCell::new(None) };
    /// States made by meow-register-state: name -> (mode, lighter).
    static STATES: RefCell<BTreeMap<String, (String, String)>> = const { RefCell::new(BTreeMap::new()) };
    /// The keys the keypad has taken so far, to undo the last one.
    static KEYPAD_SEQ: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static INDICATOR: Cell<bool> = const { Cell::new(true) };
}

pub fn remember_keypad(seq: &[String]) {
    KEYPAD_SEQ.with(|s| *s.borrow_mut() = seq.to_vec());
}

pub const BEACON_MODE: &str = "meow-beacon-mode";

// ------------------------------------------------------- pure beacon text

/// Every occurrence of `needle` in `text` (chars, not overlapping).
pub fn occurrences(text: &[char], needle: &[char]) -> Vec<Mark> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= text.len() {
        if text[i..i + needle.len()] == *needle {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// One empty-width mark at the start column of the region on each of its
/// lines, or (when the region is inside one line) every occurrence of its
/// text.
pub fn beacon_marks(text: &[char], start: usize, end: usize) -> Vec<Mark> {
    let (start, end) = (start.min(end), start.max(end));
    let sel = &text[start.min(text.len())..end.min(text.len())];
    if sel.contains(&'\n') {
        // Line-wise: one mark per line, the column of the first line.
        let line_start = text[..start].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
        let col = start - line_start;
        let mut marks = Vec::new();
        let mut ls = line_start;
        loop {
            let le = text[ls..].iter().position(|c| *c == '\n').map(|i| ls + i).unwrap_or(text.len());
            let at = (ls + col).min(le);
            if ls > end {
                break;
            }
            marks.push((at, le.min(if le >= end { end } else { le })));
            if le >= text.len() || le >= end {
                break;
            }
            ls = le + 1;
        }
        marks
    } else {
        occurrences(text, sel)
    }
}

/// What the beacon does at every mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Put `text` at the start of each mark.
    InsertAtStart(String),
    /// Put `text` at the end of each mark.
    InsertAtEnd(String),
    /// Replace each mark's text.
    Replace(String),
    /// Delete each mark's text.
    Delete,
}

/// Apply `op` at every mark; the new text and the new marks.
pub fn apply_op(text: &[char], marks: &[Mark], op: &Op) -> (String, Vec<Mark>) {
    let mut out = String::new();
    let mut new_marks = Vec::new();
    let mut at = 0;
    let mut sorted = marks.to_vec();
    sorted.sort();
    for (s, e) in sorted {
        let (s, e) = (s.min(text.len()), e.min(text.len()));
        if s < at {
            continue;
        }
        out.extend(&text[at..s]);
        let n0 = out.chars().count();
        match op {
            Op::InsertAtStart(t) => {
                out.push_str(t);
                out.extend(&text[s..e]);
            }
            Op::InsertAtEnd(t) => {
                out.extend(&text[s..e]);
                out.push_str(t);
            }
            Op::Replace(t) => out.push_str(t),
            Op::Delete => {}
        }
        new_marks.push((n0, out.chars().count()));
        at = e;
    }
    out.extend(&text[at..]);
    (out, new_marks)
}

// ------------------------------------------------------------ beacon glue

fn show_marks(b: &Buffer, marks: &[Mark]) {
    let decos: Vec<Decoration> = marks
        .iter()
        .map(|(s, e)| {
            let e = if e > s { *e } else { s + 1 };
            Decoration { span: Span { start: position_at(b, *s), end: position_at(b, e) }, face: "highlight".into(), message: None }
        })
        .collect();
    let _ = decorations::set(b, &decos);
}

fn beacon_marks_now() -> Result<(Buffer, Vec<Mark>), String> {
    let b = cur();
    match BEACON.with(|s| s.borrow().clone()) {
        Some((id, marks)) if id == b.id() => Ok((b, marks)),
        _ => Err("The beacon is not on in this buffer (meow-beacon-start)".into()),
    }
}

fn beacon_start() -> Result<(), String> {
    let b = cur();
    let text = flat_text(&b);
    let (s, e) = match b.region() {
        Some(r) => (offset_of(&b, r.start), offset_of(&b, r.end)),
        None => {
            let p = b.point();
            let (ws, we) = word_bounds(&b.line(p.line), p.column as usize);
            (offset_of(&b, pos(p.line, ws as u32)), offset_of(&b, pos(p.line, we as u32)))
        }
    };
    let marks = beacon_marks(&text, s, e);
    if marks.is_empty() {
        return Err("Nothing to put the beacon on".into());
    }
    cancel(&b);
    show_marks(&b, &marks);
    BEACON.with(|st| *st.borrow_mut() = Some((b.id(), marks.clone())));
    let _ = b.enable_minor_mode(BEACON_MODE);
    message(&format!("Beacon on {} places: i a c C r d k, n/p move, ESC quits", marks.len()));
    Ok(())
}

fn beacon_quit() {
    let b = cur();
    let _ = decorations::set(&b, &[]);
    b.disable_minor_mode(BEACON_MODE);
    BEACON.with(|st| *st.borrow_mut() = None);
}

fn beacon_do(op: Op) -> Result<(), String> {
    let (b, marks) = beacon_marks_now()?;
    let text = flat_text(&b);
    let (new, new_marks) = apply_op(&text, &marks, &op);
    let all = Span { start: pos(0, 0), end: position_at(&b, text.len()) };
    // Keep point where it was, as near as the edit allows.
    let point = offset_of(&b, b.point());
    b.replace(all, &new);
    b.set_point(position_at(&b, point.min(new.chars().count())));
    BEACON.with(|st| *st.borrow_mut() = Some((b.id(), new_marks.clone())));
    show_marks(&b, &new_marks);
    Ok(())
}

fn beacon_ask(prompt: &'static str, make: fn(String) -> Op, then_quit: bool) -> Result<(), String> {
    beacon_marks_now()?;
    read_string(prompt, "", move |s| {
        let Some(s) = s else { return };
        if let Err(e) = beacon_do(make(s)) {
            message(&e);
        } else if then_quit {
            beacon_quit();
        }
    });
    Ok(())
}

fn beacon_step(forward: bool) -> Result<(), String> {
    let (b, marks) = beacon_marks_now()?;
    let here = offset_of(&b, b.point());
    let target = if forward { marks.iter().find(|m| m.0 > here) } else { marks.iter().rev().find(|m| m.0 < here) };
    let target = target.or(if forward { marks.first() } else { marks.last() });
    if let Some((s, _)) = target {
        b.set_point(position_at(&b, *s));
    }
    Ok(())
}

fn first_mark_text(b: &Buffer, marks: &[Mark]) -> Option<String> {
    let (s, e) = *marks.first()?;
    (e > s).then(|| b.text_in(Span { start: position_at(b, s), end: position_at(b, e) }))
}

// ----------------------------------------------------------- thing help

pub fn thing_help() -> String {
    let mut s = String::from("Meow things (used by , . [ ] after a selection):\n\n");
    for (k, name) in things() {
        s.push_str(&format!("  {k}   {name}\n"));
    }
    s.push_str("\n,  select the inside of a thing     .  select its bounds\n[  go to the start of a thing        ]  go to its end\n");
    s
}

fn show_buffer(name: &str, text: &str) {
    let b = Buffer::find(name).unwrap_or_else(|| Buffer::create(name));
    b.replace(Span { start: pos(0, 0), end: position_at(&b, usize::MAX / 2) }, text);
    b.set_point(pos(0, 0));
    b.show();
}

// ---------------------------------------------------------------- init

pub fn init() -> Result<(), String> {
    modes::define_minor_mode(BEACON_MODE)?;
    modes::set_lighter(BEACON_MODE, "[B]")?;
    // ------------------------------------------------------ shortcuts
    command("meow-indent", "Indent the region, or the current line.", |_| {
        if cur().region().is_some() { run_command("indent-region") } else { run_command("indent-for-tab-command") }
    })?;
    command("meow-back-to-indentation", "Go to the first non-blank character of the line.", |_| {
        cancel(&cur());
        run_command("back-to-indentation")
    })?;
    command("meow-comment", "Comment or uncomment the region or line.", |_| run_command("comment-dwim"))?;
    command("meow-M-x", "Run a command by name.", |_| run_command("execute-extended-command"))?;
    command("meow-eval-last-exp", "Evaluate an expression (Forge has no Lisp: this asks for Lua).", |_| {
        run_command("eval-last-sexp").or_else(|_| run_command("eval-expression"))
    })?;
    // ------------------------------------------------------ beacon
    command("meow-beacon-start", "Mark every place the selection (or word) occurs, or each line of a multi-line selection, and act on them all.", |_| beacon_start())?;
    command("meow-beacon-mode", "Turn the beacon on (with the selection) or off.", |_| {
        if BEACON.with(|s| s.borrow().is_some()) {
            beacon_quit();
            Ok(())
        } else {
            beacon_start()
        }
    })?;
    command("meow-beacon-quit", "Turn the beacon off.", |_| {
        beacon_quit();
        Ok(())
    })?;
    command("meow-beacon-insert", "Insert typed text before every beacon place.", |_| beacon_ask("Insert at all places: ", Op::InsertAtStart, true))?;
    command("meow-beacon-append", "Insert typed text after every beacon place.", |_| beacon_ask("Append at all places: ", Op::InsertAtEnd, true))?;
    command(
        "meow-beacon-change",
        "Replace the text of every beacon place with typed text.\n\nWith no selection this runs the fallback for meow-beacon-change, which is meow-beacon-change-char unless you set another.",
        |_| {
            with_selection_fallback("meow-beacon-change", || {
                beacon_ask("Change all to: ", Op::Replace, true)
            })
        },
    )?;
    command(
        "meow-beacon-change-save",
        "Like meow-beacon-change, after saving the first place's text.\n\nWith no selection this says \"No selection\", as Meow does.",
        |_| {
            with_selection_fallback("meow-beacon-change-save", || {
                let (b, marks) = beacon_marks_now()?;
                if let Some(t) = first_mark_text(&b, &marks) {
                    record_kill(t);
                }
                beacon_ask("Change all to: ", Op::Replace, true)
            })
        },
    )?;
    command("meow-beacon-change-char", "Replace the character at every beacon place with typed text.", |_| {
        let (b, marks) = beacon_marks_now()?;
        let one: Vec<Mark> = marks.iter().map(|(s, _)| (*s, *s + 1)).collect();
        BEACON.with(|st| *st.borrow_mut() = Some((b.id(), one)));
        beacon_ask("Change char to: ", Op::Replace, true)
    })?;
    command("meow-beacon-replace", "Replace every beacon place with the latest saved text.", |_| {
        beacon_marks_now()?;
        let Some(text) = KILL_RING.with(|r| r.borrow().first().cloned()) else {
            return Err("Meow kill ring is empty".into());
        };
        beacon_do(Op::Replace(text.trim_end_matches('\n').to_string()))?;
        beacon_quit();
        Ok(())
    })?;
    command("meow-beacon-kill-delete", "Delete the text of every beacon place, saving the first.", |_| {
        let (b, marks) = beacon_marks_now()?;
        if let Some(t) = first_mark_text(&b, &marks) {
            record_kill(t);
        }
        beacon_do(Op::Delete)?;
        beacon_quit();
        Ok(())
    })?;
    command("meow-beacon-insert-exit", "Leave the beacon (the text was already put everywhere).", |_| {
        beacon_quit();
        Ok(())
    })?;
    command("meow-beacon-next", "Move to the next beacon place.", |_| beacon_step(true))?;
    command("meow-beacon-prev", "Move to the previous beacon place.", |_| beacon_step(false))?;
    for (cmd, doc) in [
        ("meow-beacon-apply-kmacro", "Run the last keyboard macro at every beacon place."),
        ("meow-beacon-end-and-apply-kmacro", "Stop recording the keyboard macro and run it at every beacon place."),
    ] {
        let end_first = cmd.contains("end-and");
        command(cmd, doc, move |_| {
            let (b, marks) = beacon_marks_now()?;
            if end_first {
                let _ = run_command("end-kbd-macro");
            }
            // From the bottom up, so the earlier places don't move.
            for (s, _) in marks.iter().rev() {
                b.set_point(position_at(&b, *s));
                run_command("call-last-kbd-macro")?;
            }
            beacon_quit();
            Ok(())
        })?;
    }
    for (k, c) in [("i", "meow-beacon-insert"), ("a", "meow-beacon-append"), ("c", "meow-beacon-change"), ("C", "meow-beacon-change-save"), ("s", "meow-beacon-change-char"), ("r", "meow-beacon-replace"), ("d", "meow-beacon-kill-delete"), ("k", "meow-beacon-kill-delete"), ("n", "meow-beacon-next"), ("p", "meow-beacon-prev"), ("q", "meow-beacon-quit"), ("ESC", "meow-beacon-quit"), ("g", "meow-beacon-quit"), ("m", "meow-beacon-apply-kmacro")] {
        bind_in_mode(BEACON_MODE, k, c)?;
    }
    // ------------------------------------------------------ states
    command("meow-global-mode", "Turn modal editing on or off in every buffer.", |_| run_command("meow-mode"))?;
    command("meow-motion-mode", "Switch this buffer to motion state.", |_| run_command("meow-motion-state"))?;
    command("meow-keypad-mode", "Open the keypad.", |_| run_command("meow-keypad"))?;
    command("meow-register-state", "Make a new state (a minor mode and a command to enter it).", |_| {
        read_string("State name: ", "", |name| {
            let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) else { return };
            read_string("Lighter: ", &format!("[{}]", name.chars().next().unwrap_or('S').to_ascii_uppercase()), move |l| {
                let lighter = l.unwrap_or_else(|| "[S]".into());
                let mode = format!("meow-{name}-mode");
                let cmd = format!("meow-{name}-state");
                let r = (|| {
                    modes::define_minor_mode(&mode)?;
                    modes::set_lighter(&mode, &lighter)?;
                    let m2 = mode.clone();
                    command(&cmd, &format!("Switch this buffer to {name} state."), move |_| {
                        let b = cur();
                        b.disable_minor_mode(NORMAL);
                        b.disable_minor_mode(INSERT);
                        b.disable_minor_mode(MOTION);
                        b.disable_minor_mode(webkit::MODE);
                        b.disable_minor_mode(LEADER);
                        for (m, _) in STATES.with(|s| s.borrow().values().cloned().collect::<Vec<_>>()) {
                            b.disable_minor_mode(&m);
                        }
                        b.enable_minor_mode(&m2)
                    })?;
                    bind_in_mode(&mode, "ESC", "meow-normal-state")
                })();
                match r {
                    Ok(()) => {
                        STATES.with(|s| s.borrow_mut().insert(name.clone(), (mode.clone(), lighter)));
                        message(&format!("State {name} registered: M-x {cmd}; bind keys with meow-define-key"));
                    }
                    Err(e) => message(&e),
                }
            });
        });
        Ok(())
    })?;
    command("meow-define-key", "Bind a key in a Meow state: asks for the state (normal, motion, leader, insert or a registered one), the key and the command.", |_| {
        let mut states: Vec<String> = vec!["normal".into(), "motion".into(), "leader".into(), "insert".into(), "beacon".into()];
        states.extend(STATES.with(|s| s.borrow().keys().cloned().collect::<Vec<_>>()));
        forge_plugin_sdk::completing_read("State: ", &states, true, |st| {
            let Some(st) = st else { return };
            read_string("Key: ", "", move |key| {
                let Some(key) = key.filter(|k| !k.trim().is_empty()) else { return };
                read_string("Command: ", "", move |c| {
                    let Some(c) = c.filter(|c| !c.trim().is_empty()) else { return };
                    let mode = match st.as_str() {
                        "normal" => NORMAL.to_string(),
                        "motion" => MOTION.to_string(),
                        "leader" => LEADER.to_string(),
                        "insert" => INSERT.to_string(),
                        "beacon" => BEACON_MODE.to_string(),
                        other => format!("meow-{other}-mode"),
                    };
                    match bind_in_mode(&mode, key.trim(), c.trim()) {
                        Ok(()) => message(&format!("{} in {st} state runs {}", key.trim(), c.trim())),
                        Err(e) => message(&e),
                    }
                });
            });
        });
        Ok(())
    })?;
    for (cmd, st) in [("meow-normal-define-key", "normal"), ("meow-motion-define-key", "motion"), ("meow-leader-define-key", "leader")] {
        command(cmd, &format!("Bind a key in {st} state."), move |_| {
            read_string("Key: ", "", move |key| {
                let Some(key) = key.filter(|k| !k.trim().is_empty()) else { return };
                read_string("Command: ", "", move |c| {
                    let Some(c) = c.filter(|c| !c.trim().is_empty()) else { return };
                    let mode = match st {
                        "normal" => NORMAL,
                        "motion" => MOTION,
                        _ => LEADER,
                    };
                    let bound = if st == "motion" {
                        bind_motion(key.trim(), c.trim())
                    } else {
                        bind_in_mode(mode, key.trim(), c.trim())
                    };
                    match bound {
                        Ok(()) => message(&format!("{} in {st} state runs {}", key.trim(), c.trim())),
                        Err(e) => message(&e),
                    }
                });
            });
            Ok(())
        })?;
    }
    // ------------------------------------------------------ keypad & help
    command("meow-keypad-undo", "Take back the last key typed in the keypad.", |_| {
        let seq = KEYPAD_SEQ.with(|s| {
            let mut s = s.borrow_mut();
            s.pop();
            s.clone()
        });
        quit_keypad();
        if seq.is_empty() {
            return open_keypad();
        }
        keypad(seq, false, None);
        Ok(())
    })?;
    command("meow-describe-key", "Say what a key is bound to.", |_| {
        describe_key();
        Ok(())
    })?;
    command("meow-describe-keymap", "List the keys of normal state.", |_| {
        let mut s = String::from("Meow normal state:\n\n");
        let name = LAYOUT.with(Cell::get);
        if let Some(layout) = layout::layout(name) {
            for (k, c) in layout::normal_keys(layout) {
                s.push_str(&format!("  {k:<6} {c}\n"));
            }
        }
        show_buffer("*Meow Keymap*", &s);
        Ok(())
    })?;
    command("meow-display-thing-help", "Show the things Meow can select.", |_| {
        show_buffer("*Meow Things*", &thing_help());
        Ok(())
    })?;
    command("meow-char-thing-table", "Show the keys of the thing table.", |_| {
        let t: String = things()
            .into_iter()
            .map(|(k, n)| format!("{k} -> {n}\n"))
            .collect();
        show_buffer("*Meow Thing Table*", &t);
        Ok(())
    })?;
    // ------------------------------------------------------ display
    command("meow-update-display", "Redo the state lighters and cursor of every buffer.", |_| {
        for b in Buffer::all() {
            if b.alive() && ENABLED.with(Cell::get) {
                let mode = b.minor_modes();
                if mode.contains(&INSERT.to_string()) {
                    let _ = b.set_option("cursor", "bar");
                } else if mode.contains(&NORMAL.to_string()) {
                    let _ = b.set_option("cursor", "box");
                }
            }
        }
        Ok(())
    })?;
    command("meow-setup-indicator", "Toggle the [N]/[I] state indicator in the mode line.", |_| {
        let on = !INDICATOR.with(Cell::get);
        INDICATOR.with(|i| i.set(on));
        let (n, i, m) = if on { ("[N]", "[I]", "[M]") } else { ("", "", "") };
        modes::set_lighter(NORMAL, n)?;
        modes::set_lighter(INSERT, i)?;
        modes::set_lighter(MOTION, m)?;
        modes::set_lighter(webkit::MODE, m)?;
        message(if on { "State indicator on" } else { "State indicator off" });
        Ok(())
    })?;
    command("meow-setup-line-number", "Show relative line numbers (Meow's setup) in this buffer, or turn them off.", |_| {
        let b = cur();
        let now = b.get_option("line-numbers").unwrap_or_default();
        b.set_option("line-numbers", if now == "relative" { "" } else { "relative" })
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn marks_for_a_word_are_its_occurrences() {
        let t = c("foo bar foo foobar");
        assert_eq!(beacon_marks(&t, 0, 3), vec![(0, 3), (8, 11), (12, 15)]);
        assert_eq!(beacon_marks(&t, 4, 7), vec![(4, 7), (15, 18)].into_iter().filter(|m| t[m.0..m.1] == c("bar")[..]).collect::<Vec<_>>());
    }

    #[test]
    fn marks_for_lines_are_one_per_line() {
        let t = c("ab\ncd\nef\ngh");
        // Lines 0-2 selected from the start of line 0.
        let m = beacon_marks(&t, 0, 8);
        assert_eq!(m.len(), 3, "{m:?}");
        assert_eq!(m.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 3, 6]);
    }

    #[test]
    fn ops_apply_everywhere_and_track_marks() {
        let t = c("a x b x c");
        let marks = vec![(2, 3), (6, 7)];
        let (s, m) = apply_op(&t, &marks, &Op::Replace("YY".into()));
        assert_eq!(s, "a YY b YY c");
        assert_eq!(m, vec![(2, 4), (7, 9)]);
        let (s, _) = apply_op(&t, &marks, &Op::InsertAtStart("<".into()));
        assert_eq!(s, "a <x b <x c");
        let (s, _) = apply_op(&t, &marks, &Op::InsertAtEnd(">".into()));
        assert_eq!(s, "a x> b x> c");
        let (s, m) = apply_op(&t, &marks, &Op::Delete);
        assert_eq!(s, "a  b  c");
        assert_eq!(m, vec![(2, 2), (5, 5)]);
    }

    #[test]
    fn thing_help_lists_every_thing() {
        let h = thing_help();
        for (k, n) in things() {
            assert!(h.contains(&format!("  {k}   {n}")), "{n}");
        }
    }
}
