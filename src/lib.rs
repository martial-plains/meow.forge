// SPDX-License-Identifier: GPL-3.0-or-later
//! meow for Forge: modal editing in the spirit of Emacs's meow (not Vim
//! emulation). Selection first: motions make or extend the region, and
//! commands act on it. Emacs's own keys stay reachable through the keypad
//! (SPC), so nothing needs a chord.
//!
//! Normal state (`[N]`, box cursor), QWERTY layout:
//!
//! ```text
//! h j k l   left, down, up, right (cancel the selection)
//! w         select the word at point      e / b  select to the next / previous word
//! x         select the line (again: one more line)
//! f / t     select to / till a character (reads it)
//! ;         swap the selection's ends     g      cancel the selection
//! d         kill the selection (or the character)
//! c         change: kill it and insert    y      copy    p  paste
//! i / a     insert before / after         o / O  open a line below / above
//! u         undo                          /      search    :  M-x
//! SPC       keypad: SPC x f is C-x C-f, SPC m f is M-f, SPC g x is C-M-x,
//!           SPC <letter> is C-c <letter>, SPC SPC <key> a plain key
//! ```
//!
//! Insert state (`[I]`, bar cursor): type as usual; ESC goes back to
//! normal. Special modes (help, Dired, WebKit pages) start in motion state:
//! their own keys keep working, and SPC opens the keypad.

use forge_plugin_sdk::{
    bind_in_mode, bind_transient, command as sdk_command, completing_read, execute_keys,
    forge_plugin, keymap, kill_ring, message, modes, on, pop_transient, push_transient, read_key,
    read_string, run_command, Buffer, EventKind, Position, Span, TransientOpts,
};
use std::cell::{Cell, RefCell};

mod config;
mod extra;
mod layout;
mod webkit;

pub const NORMAL: &str = "meow-normal-mode";
pub const INSERT: &str = "meow-insert-mode";
/// Where a count is typed before a normal-state key.
pub const LEADER: &str = "meow-leader-mode";
/// Where a special mode's own keys stay, with SPC opening the keypad.
pub const MOTION: &str = "meow-motion-mode";

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static SEARCH_RING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static KILL_RING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static KILL_RING_INDEX: Cell<usize> = const { Cell::new(0) };
    static SELECTION_RING: RefCell<Vec<(u32, Position, Position)>> = const { RefCell::new(Vec::new()) };
    static MARK_RING: RefCell<Vec<(u32, Position)>> = const { RefCell::new(Vec::new()) };
    static GRAB: RefCell<Option<(u32, Position, Position, String)>> = const { RefCell::new(None) };
    /// The character of the last find or till, for meow-find-ref.
    static LAST_CHAR: Cell<Option<char>> = const { Cell::new(None) };
    /// Whether the last mark command jumped away (so the next one should
    /// jump back), for meow-pop-or-unpop-to-mark.
    static MARK_JUMP: Cell<bool> = const { Cell::new(false) };
    /// Where the last meow yank (or replace) put its text, so that
    /// meow-yank-pop can take it out again: (buffer, start, end).
    static LAST_YANK: Cell<Option<(u32, Position, Position)>> = const { Cell::new(None) };
    /// The selection the last meow-line made, so that x extends it only
    /// when it really is whole lines: (buffer, start, end).
    static LINE_SEL: Cell<Option<(u32, Position, Position)>> = const { Cell::new(None) };
    /// Why insert state was entered, so that leaving it can select what was
    /// typed: `meow-select-on-{insert,append,change,open}`.
    static INSERT_CAUSE: Cell<Option<&'static str>> = const { Cell::new(None) };
    /// Where that insertion started, for the same reason.
    static INSERT_START: Cell<Option<Position>> = const { Cell::new(None) };
    /// The name of the last Meow command that ran. Meow's
    /// The command that ran last, which is the one Meow's `last-command`
    /// would be: `meow-replace-pop` starts in the kill ring where the
    /// command before it started, so it needs to know what that was.
    static LAST_COMMAND: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The command before that one, which is what Emacs's `last-command`
    /// says while a command is running.
    static PREVIOUS_COMMAND: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Where the last meow-replace put its text, so that meow-replace-pop
    /// can take it out again: (buffer, start, end).
    static REPLACE_START: Cell<Option<(u32, Position, Position)>> = const { Cell::new(None) };
    /// How far through the kill ring meow-replace-pop is.
    static REPLACE_POP_INDEX: Cell<usize> = const { Cell::new(0) };
    /// Whether the last meow-pop-or-unpop-to-mark jumped away, so that
    /// doing it again comes back:
    /// `meow-pop-or-unpop-to-mark-repeat-unpop`.
    static MARK_UNPOP: Cell<bool> = const { Cell::new(false) };
    /// What the current selection is ("word", "symbol", "line", "char"),
    /// so that meow-expand grows by that and not by a character.
    static SELECTION_KIND: Cell<&'static str> = const { Cell::new("char") };
    /// Whether the current selection is an expansion of another one, which
    /// is what `meow-expand-selection-type` decides.
    static EXPANDED: Cell<bool> = const { Cell::new(false) };
    /// Which keymap layout is in use.
    static LAYOUT: Cell<&'static str> = const { Cell::new("qwerty") };
}

/// Register a Meow command. The name is remembered while it runs, because
/// `meow-replace-pop` starts in the kill ring where the command before it
/// started, and that is the command's name.
fn command(
    name: &str,
    doc: &str,
    run: impl Fn(Option<i64>) -> Result<(), String> + 'static,
) -> Result<(), String> {
    let name = name.to_string();
    let command_name = name.clone();
    sdk_command(&name, doc, move |n| {
        LAST_COMMAND.with(|c| {
            let mut c = c.borrow_mut();
            let old = c.clone();
            PREVIOUS_COMMAND.with(|p| *p.borrow_mut() = old);
            *c = Some(command_name.clone());
        });
        run(n)
    })
}

/// The command that ran before this one, which is Emacs's `last-command`.
fn previous_command() -> Option<String> {
    PREVIOUS_COMMAND.with(|c| c.borrow().clone())
}

fn same(a: Position, b: Position) -> bool {
    (a.line, a.column) == (b.line, b.column)
}

fn is_before(a: Position, b: Position) -> bool {
    (a.line, a.column) < (b.line, b.column)
}

/// The selection's ends in buffer order, or point twice with none.
fn selection_bounds(b: &Buffer) -> (Position, Position) {
    match b.region() {
        Some(r) => (r.start, r.end),
        None => (b.point(), b.point()),
    }
}

/// Insert yanked text at point and remember where it went.
fn insert_yank(b: &Buffer, text: &str) {
    let start = b.point();
    b.insert(text);
    LAST_YANK.with(|last| last.set(Some((b.id(), start, b.point()))));
}

/// The last match that starts before `before`. `search(from)` finds the first
/// match starting at or after `from` (the buffer has no backward search);
/// `step` moves a position one character on.
pub fn previous_match(
    mut search: impl FnMut(Position) -> Option<Span>,
    step: impl Fn(Position) -> Position,
    before: Position,
) -> Option<Span> {
    let mut from = Position { line: 0, column: 0 };
    let mut last = None;
    for _ in 0..100_000 {
        let Some(span) = search(from) else { break };
        if !is_before(span.start, before) {
            break;
        }
        let next = step(span.start);
        last = Some(span);
        if same(next, from) {
            break;
        }
        from = next;
    }
    last
}

/// Where `f` / `t` land: the offset to put point at, given `count` and
/// the direction. Forward `f` goes just past the character and `t` stops
/// before it; backward `f` goes onto it and `t` stops after it, so the
/// selection from the old point includes the character only for `f`.
pub fn find_target(text: &[char], start: usize, c: char, count: usize, backward: bool, include: bool) -> Option<usize> {
    let count = count.max(1);
    let hits = text.iter().enumerate().filter(|(_, x)| **x == c).map(|(i, _)| i);
    if backward {
        let index = hits.filter(|i| *i < start).last_n(count)?;
        Some(if include { index } else { index + 1 })
    } else {
        let index = hits.filter(|i| *i > start).nth(count - 1)?;
        Some(if include { index + 1 } else { index })
    }
}

/// Where meow-find-ref lands: past the next `c`, or onto the one before.
pub fn find_ref(text: &[char], at: usize, c: char, backward: bool) -> Option<usize> {
    if backward {
        text[..at.min(text.len())].iter().rposition(|x| *x == c)
    } else {
        text.iter().enumerate().skip(at.saturating_add(1)).find(|(_, x)| **x == c).map(|(i, _)| i + 1)
    }
}

/// The `n`th item from the end, for the backward find above.
trait LastN: Iterator + Sized {
    fn last_n(self, n: usize) -> Option<Self::Item> {
        let all: Vec<Self::Item> = self.collect();
        all.len().checked_sub(n).and_then(|i| all.into_iter().nth(i))
    }
}
impl<I: Iterator> LastN for I {}

/// The lines a whole-line kill takes: the lines the selection touches (a
/// selection that ends at the start of a line stops short of it), or the
/// line point is on, each with its newline, except the buffer's last line,
/// which has none.
pub fn whole_lines(
    region: Option<(Position, Position)>,
    point_line: u32,
    line_count: u32,
    line_len: impl Fn(u32) -> u32,
) -> (Position, Position) {
    let (first, last) = match region {
        Some((s, e)) => (s.line, if e.column == 0 && e.line > s.line { e.line - 1 } else { e.line }),
        None => (point_line, point_line),
    };
    let end = if last + 1 < line_count {
        Position { line: last + 1, column: 0 }
    } else {
        Position { line: last, column: line_len(last) }
    };
    (Position { line: first, column: 0 }, end)
}

/// Swap two stretches of the text: the plan, and where each one's text is
/// afterwards (`a`'s text first). None when they overlap.
pub fn plan_swap(text: &[char], a: (usize, usize), b: (usize, usize)) -> Option<(Plan, (usize, usize), (usize, usize))> {
    let a_first = a.0 <= b.0;
    let (first, second) = if a_first { (a, b) } else { (b, a) };
    if first.1 > second.0 {
        return None;
    }
    let t1 = slice(text, first.0, first.1);
    let t2 = slice(text, second.0, second.1);
    let gap = slice(text, first.1, second.0);
    let (l1, l2, lg) = (t1.chars().count(), t2.chars().count(), gap.chars().count());
    let first_now = (first.0 + l2 + lg, first.0 + l2 + lg + l1);
    let second_now = (first.0, first.0 + l2);
    let plan = Plan::replace((first.0, second.1), format!("{t2}{gap}{t1}"));
    Some(if a_first { (plan, first_now, second_now) } else { (plan, second_now, first_now) })
}

/// Go to the mark on top of the mark ring, remembering where we were so
/// `meow-unpop-to-mark` can come back. With `same_buffer` the ring is
/// searched from the top for a mark in this buffer, which is what a
/// buffer-local mark jump means; without it the top of the ring wins.
fn jump_to_mark(same_buffer: bool) -> Result<(), String> {
    let b = cur();
    let here = (b.id(), b.point());
    let found = MARK_RING.with(|ring| {
        let mut ring = ring.borrow_mut();
        let at = if same_buffer {
            ring.iter().rposition(|(id, _)| *id == here.0)
        } else {
            (!ring.is_empty()).then_some(ring.len() - 1)
        };
        match at {
            Some(at) => Some(ring.remove(at)),
            None => None,
        }
    });
    match found {
        Some((id, point)) => {
            MARK_RING.with(|ring| ring.borrow_mut().push(here));
            MARK_JUMP.with(|jumped| jumped.set(true));
            match Buffer::from_id(id) {
                Some(b) => {
                    b.show();
                    b.set_point(point);
                }
                None => message("The buffer for that mark was closed"),
            }
        }
        None => {
            MARK_JUMP.with(|jumped| jumped.set(false));
            message("No Meow mark");
        }
    }
    Ok(())
}

/// Come back from `jump_to_mark`: take the mark we pushed and go there,
/// leaving the ring as it was.
fn unjump_to_mark() -> Result<(), String> {
    let previous = MARK_RING.with(|ring| ring.borrow_mut().pop());
    match previous {
        Some((id, point)) => {
            MARK_JUMP.with(|jumped| jumped.set(false));
            match Buffer::from_id(id) {
                Some(b) => {
                    b.show();
                    b.set_point(point);
                }
                None => message("The buffer for that mark was closed"),
            }
        }
        None => message("No Meow mark"),
    }
    Ok(())
}

/// `meow-open-below`/`-above`. In the visual form the line is the one the
/// selection starts on and point keeps its column on the new line.
fn open_line(below: bool, visual: bool) -> Result<(), String> {
    let b = cur();
    let (line, column) = match (visual, b.region()) {
        (true, Some(region)) => (region.start.line, region.start.column),
        _ => {
            let p = b.point();
            (p.line, p.column)
        }
    };
    cancel(&b);
    if below {
        let end = char_len(&b.line(line));
        b.set_point(pos(line, end));
        b.insert("\n");
        b.indent_line(line + 1);
        b.set_point(pos(line + 1, column));
    } else {
        b.set_point(pos(line, 0));
        b.insert("\n");
        b.set_point(pos(line, 0));
        b.indent_line(line);
    }
    insert_for(&b, "open");
    Ok(())
}

// ------------------------------------------------------------ states

/// The state a buffer starts in: the closest mode in `meow-mode-state-list`,
/// then motion for special-mode descendants, normal otherwise.
///
/// Emacs guesses from the keys: if a letter self-inserts, normal, else
/// motion. Forge does not bind self-insert to keys, so the mode lineage is
/// the same signal: a special mode already has keys of its own, and motion
/// state leaves them alone.
fn state_for(b: &Buffer) -> String {
    state_for_mode(&b.major_mode(), |mode| modes::major_mode_parent(mode))
}

fn state_for_mode(mode: &str, parent: impl Fn(&str) -> Option<String>) -> String {
    config::with(|o| {
        let mut current = Some(mode.to_string());
        for _ in 0..32 {
            let Some(name) = current else { break };
            if let Some(state) = o.configured_state(&name) {
                return state.to_string();
            }
            current = parent(&name);
        }
        let mut current = Some(mode.to_string());
        for _ in 0..32 {
            let Some(name) = current else { break };
            if name == "special-mode" {
                return "motion".to_string();
            }
            current = parent(&name);
        }
        "normal".to_string()
    })
}

fn normal(b: &Buffer) {
    b.disable_minor_mode(INSERT);
    b.disable_minor_mode(MOTION);
    b.disable_minor_mode(LEADER);
    b.disable_minor_mode(webkit::MODE);
    let _ = b.enable_minor_mode(NORMAL);
    let _ = b.set_option("cursor", "box");
}

fn insert(b: &Buffer) {
    b.disable_minor_mode(NORMAL);
    b.disable_minor_mode(MOTION);
    b.disable_minor_mode(LEADER);
    b.disable_minor_mode(webkit::MODE);
    let _ = b.enable_minor_mode(INSERT);
    let _ = b.set_option("cursor", "bar");
}

/// Enter insert state, remembering why, so that leaving it can select what
/// was typed: `meow-select-on-{insert,append,change,open}`.
fn insert_for(b: &Buffer, cause: &'static str) {
    INSERT_CAUSE.with(|c| c.set(Some(cause)));
    INSERT_START.with(|s| s.set(Some(b.point())));
    insert(b);
}

/// Select what was typed in insert state, if the user asked for that.
fn select_inserted(b: &Buffer) {
    let Some(cause) = INSERT_CAUSE.with(Cell::get) else {
        return;
    };
    let wanted = config::with(|o| match cause {
        "insert" => o.select_on_insert,
        "append" => o.select_on_append,
        "change" => o.select_on_change,
        "open" => o.select_on_open,
        _ => false,
    });
    if !wanted {
        return;
    }
    let Some(start) = INSERT_START.with(Cell::get) else {
        return;
    };
    let end = b.point();
    if same(start, end) {
        return;
    }
    b.set_mark(Some(start));
    b.set_point(end);
}

fn motion(b: &Buffer) {
    b.disable_minor_mode(INSERT);
    b.disable_minor_mode(NORMAL);
    b.disable_minor_mode(LEADER);
    // A web page's motion state is the Vim-style key layer standing in for plain
    // motion state (it has the keypad and everything else motion has), so
    // the buffer is in one state, not two.
    if webkit::applies(&b.major_mode()) {
        b.disable_minor_mode(MOTION);
        let _ = b.enable_minor_mode(webkit::MODE);
    } else {
        b.disable_minor_mode(webkit::MODE);
        let _ = b.enable_minor_mode(MOTION);
    }
}

/// Bind a key in motion state, and in the web page flavour of it, unless
/// the Vim-style layer already uses that key.
fn bind_motion(key: &str, command: &str) -> Result<(), String> {
    bind_in_mode(MOTION, key, command)?;
    if !webkit::KEYS.iter().any(|(k, _)| *k == key) {
        let _ = bind_in_mode(webkit::MODE, key, command);
    }
    Ok(())
}

fn off(b: &Buffer) {
    b.disable_minor_mode(webkit::MODE);
    b.disable_minor_mode(NORMAL);
    b.disable_minor_mode(INSERT);
    b.disable_minor_mode(MOTION);
    b.disable_minor_mode(LEADER);
    let _ = b.set_option("cursor", "");
}

fn setup(b: &Buffer) {
    if ENABLED.with(Cell::get) && b.alive() {
        let modes = b.minor_modes();
        if !modes.contains(&NORMAL.to_string())
            && !modes.contains(&INSERT.to_string())
            && !modes.contains(&MOTION.to_string())
            && !modes.contains(&webkit::MODE.to_string())
            && !modes.contains(&LEADER.to_string())
        {
            // The state a major mode starts in is the user's to choose:
            // `meow-mode-state-list`.
            let state = state_for(b);
            match state.as_str() {
                "motion" => motion(b),
                "insert" => insert(b),
                _ => normal(b),
            }
        }
    }
}

// --------------------------------------------------------- selection

fn cur() -> Buffer {
    Buffer::current()
}

fn pos(line: u32, column: u32) -> Position {
    Position { line, column }
}

fn char_len(s: &str) -> u32 {
    s.chars().count() as u32
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_symbol(c: char) -> bool {
    c == '_' || c == '-' || c.is_alphanumeric()
}

/// The word at (or, off a word, after) column `col`: (start, end).
pub fn word_bounds(line: &str, col: usize) -> (usize, usize) {
    let t: Vec<char> = line.chars().collect();
    let mut e = col.min(t.len());
    if !t.get(e).is_some_and(|c| is_word(*c)) {
        while e < t.len() && !is_word(t[e]) {
            e += 1;
        }
    }
    let mut s = e;
    while s > 0 && is_word(t[s - 1]) {
        s -= 1;
    }
    while e < t.len() && is_word(t[e]) {
        e += 1;
    }
    (s, e)
}

/// The symbol or compound identifier at `col`: (start, end).
pub fn symbol_bounds(line: &str, col: usize) -> (usize, usize) {
    let t: Vec<char> = line.chars().collect();
    let mut e = col.min(t.len());
    if !t.get(e).is_some_and(|c| is_symbol(*c)) {
        while e < t.len() && !is_symbol(t[e]) {
            e += 1;
        }
    }
    let mut s = e;
    while s > 0 && is_symbol(t[s - 1]) {
        s -= 1;
    }
    while e < t.len() && is_symbol(t[e]) {
        e += 1;
    }
    (s, e)
}

/// The column of the next `c` in `line` after `col` (f), or before it (t).
pub fn find_char(line: &str, from: usize, c: char, include: bool) -> Option<usize> {
    line.chars()
        .enumerate()
        .skip(from)
        .find(|(_, x)| *x == c)
        .map(|(i, _)| if include { i + 1 } else { i })
}

fn cancel(b: &Buffer) {
    b.set_mark(None);
}

/// Meow's `meow--with-selection-fallback`: do `body` when there is a
/// selection, and otherwise run the fallback the user set for `command`
/// (`meow-selection-command-fallback`) — or say that there is no
/// selection, as Meow does.
fn with_selection_fallback(
    command: &str,
    body: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if cur().region().is_some() {
        return body();
    }
    let fallback = config::with(|o| o.fallback(command).map(str::to_string));
    match fallback {
        Some(fallback) => run_command(&fallback),
        None => Err("No selection".into()),
    }
}

/// The texts a yank can take: the editor's own kill ring (Forge's
/// clipboard history) when the user asked for the clipboard, else Meow's
/// own ring.
fn kill_ring_items() -> Vec<String> {
    if config::with(|o| o.use_clipboard) {
        kill_ring::entries()
    } else {
        KILL_RING.with(|r| r.borrow().clone())
    }
}

/// The text a yank takes now.
fn yank_text() -> Option<String> {
    if config::with(|o| o.use_clipboard) {
        kill_ring::current()
    } else {
        KILL_RING.with(|r| r.borrow().first().cloned())
    }
}

/// The text of the entry `index` steps behind the current one, in the ring
/// a yank takes from.
fn kill_ring_item(index: usize) -> Option<String> {
    kill_ring_items().get(index).cloned()
}

/// Kill the text in `span`: it goes on Meow's own ring, and — when the
/// user asked for the clipboard — on the editor's too, by letting the
/// editor kill the region.
fn kill_span(b: &Buffer, span: Span) -> Result<(), String> {
    let text = b.text_in(span);
    record_kill(text);
    if config::with(|o| o.use_clipboard) {
        // The editor's kill-region deletes the text and puts it on the
        // editor's own ring, which is the clipboard the user asked for.
        b.set_mark(Some(span.start));
        b.set_point(span.end);
        run_command("kill-region")
    } else {
        b.delete(span);
        cancel(b);
        Ok(())
    }
}

/// Push `text` onto the plugin's own kill ring, at the front, and make it
/// the one a following yank gets. Meow's ring is not the editor's, so this
/// is the only place a Meow kill is recorded.
fn record_kill(text: String) {
    KILL_RING.with(|ring| ring.borrow_mut().insert(0, text));
    KILL_RING_INDEX.with(|i| i.set(0));
}

/// Kill `n` words or symbols around point, forward or backward, and record
/// them. The editor binds `M-d`/`M-DEL` for words but nothing at all for
/// symbols, so Meow's symbol kills are done here.
fn kill_chars(symbol: bool, backward: bool, n: Option<i64>) -> Result<(), String> {
    let b = cur();
    let text = flat_text(&b);
    let start = offset_of(&b, b.point());
    let count = n.unwrap_or(1).unsigned_abs().max(1) as usize;
    let same = |c: char| if symbol { is_symbol(c) } else { is_word(c) };
    let mut at = start;
    for _ in 0..count {
        // Past whatever isn't part of the word, then over the word itself.
        if backward {
            while at > 0 && !same(text[at - 1]) {
                at -= 1;
            }
            while at > 0 && same(text[at - 1]) {
                at -= 1;
            }
        } else {
            while at < text.len() && !same(text[at]) {
                at += 1;
            }
            while at < text.len() && same(text[at]) {
                at += 1;
            }
        }
    }
    if at == start {
        message(if backward {
            "Nothing to kill backward"
        } else {
            "Nothing to kill forward"
        });
        return Ok(());
    }
    let span = Span {
        start: position_at(&b, start),
        end: position_at(&b, at),
    };
    kill_span(&b, span)
}

/// Kill the line point is on, newline and all, the way Meow's whole-line
/// kill does. Forge only has `kill-line`, which leaves the newline.
fn kill_whole_line() -> Result<(), String> {
    let b = cur();
    let region = b.region().map(|r| (r.start, r.end));
    let (start, end) = whole_lines(region, b.point().line, b.line_count(), |l| char_len(&b.line(l)));
    let span = Span { start, end };
    kill_span(&b, span)
}

/// Kill the selection and append what was killed to the text already on the
/// ring, instead of making it a new entry. Meow's ring is the plugin's, so
/// there is no `append-next-kill` to lean on.
fn kill_append() -> Result<(), String> {
    let b = cur();
    let Some(region) = b.region() else {
        message("Nothing selected");
        return Ok(());
    };
    let text = b.text_in(region);
    KILL_RING.with(|ring| {
        let mut ring = ring.borrow_mut();
        if let Some(last) = ring.first_mut() {
            last.push_str(&text);
        } else {
            ring.push(text);
        }
    });
    KILL_RING_INDEX.with(|i| i.set(0));
    if config::with(|o| o.use_clipboard) {
        // The editor's ring takes whole entries only, so the text goes on
        // as one of its own rather than onto the last one.
        b.set_mark(Some(region.start));
        b.set_point(region.end);
        run_command("kill-region")
    } else {
        b.delete(region);
        cancel(&b);
        Ok(())
    }
}

fn select_from_point(b: &Buffer) {
    if b.region().is_none() {
        b.set_mark(Some(b.point()));
    }
}

fn offset_of(b: &Buffer, p: Position) -> usize {
    let mut offset = 0;
    for line in 0..p.line.min(b.line_count()) {
        offset += b.line(line).chars().count() + 1;
    }
    offset + (p.column as usize).min(b.line(p.line).chars().count())
}

fn position_at(b: &Buffer, offset: usize) -> Position {
    let mut rest = offset;
    for line in 0..b.line_count() {
        let len = b.line(line).chars().count();
        if rest <= len {
            return pos(line, rest as u32);
        }
        rest = rest.saturating_sub(len + 1);
    }
    let line = b.line_count().saturating_sub(1);
    pos(line, char_len(&b.line(line)))
}

fn select_span(b: &Buffer, start: Position, end: Position) {
    select_span_kind(b, start, end, "char")
}

/// Make a selection, and say what it is made of: Meow expands by the same
/// thing the selection was made of.
fn select_span_kind(b: &Buffer, start: Position, end: Position, kind: &'static str) {
    if let Some(mark) = b.mark().filter(|_| b.region().is_some()) {
        SELECTION_RING.with(|ring| ring.borrow_mut().push((b.id(), mark, b.point())));
    }
    b.set_mark(Some(start));
    b.set_point(end);
    SELECTION_KIND.with(|k| k.set(kind));
    EXPANDED.with(|e| e.set(false));
}

fn select_oriented(b: &Buffer, start: Position, end: Position, backward: bool) {
    if backward {
        select_span(b, end, start);
    } else {
        select_span(b, start, end);
    }
}

/// As [`select_oriented`], for a selection of a known kind.
fn select_oriented_kind(
    b: &Buffer,
    start: Position,
    end: Position,
    backward: bool,
    kind: &'static str,
) {
    if backward {
        select_span_kind(b, end, start, kind);
    } else {
        select_span_kind(b, start, end, kind);
    }
}

/// Whether the selection is Meow's '(expand . char): an expansion of a
/// character selection, which the move commands keep.
fn expanded_char() -> bool {
    EXPANDED.with(Cell::get) && SELECTION_KIND.with(Cell::get) == "char"
}

fn flat_text(b: &Buffer) -> Vec<char> {
    b.text().chars().collect()
}

/// The words and symbols of the buffer, for `meow-visit`'s completion. How
/// short a word may be is `meow-visit-collect-min-length`.
fn visit_candidates(b: &Buffer) -> Vec<String> {
    let min = config::with(|o| o.visit_collect_min_length) as usize;
    let mut out: Vec<String> = Vec::new();
    for line in 0..b.line_count() {
        let chars: Vec<char> = b.line(line).chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if is_symbol_or_word(chars[i]) {
                let start = i;
                while i < chars.len() && is_symbol_or_word(chars[i]) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if word.chars().count() >= min && !out.contains(&word) {
                    out.push(word);
                }
            } else {
                i += 1;
            }
        }
    }
    out.sort();
    out
}

/// The text with the regular expression's own characters taken out of it,
/// so that it matches itself and nothing else.
fn escape_regexp(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&~-".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The last match of `pattern` before `before`, which the buffer can only
/// find by searching forwards.
fn search_backward(b: &Buffer, pattern: &str) -> Result<Option<Span>, String> {
    // A pattern that doesn't compile is reported as such.
    b.search_regexp(pattern, pos(0, 0))?;
    Ok(previous_match(
        |from| b.search_regexp(pattern, from).ok().flatten(),
        |p| {
            if p.column < char_len(&b.line(p.line)) {
                pos(p.line, p.column + 1)
            } else if p.line + 1 < b.line_count() {
                pos(p.line + 1, 0)
            } else {
                p
            }
        },
        b.point(),
    ))
}

/// The brackets Meow knows, and whether a `"` closes what it opens. A
/// backslash inside a string escapes the next character.
const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// The angle brackets of the QWERTZ layout, which Meow registers as a
/// thing of their own.
const ANGLE: (char, char) = (';', ':');

/// Every balanced bracket pair in `text`, as (start, end) of the whole thing,
/// outermost first, and never overlapping.
fn pairs_in(text: &[char]) -> Vec<(usize, usize)> {
    pairs_in_with(text, &PAIRS)
}

/// As [`pairs_in`], for the brackets of `pairs`.
fn pairs_in_with(text: &[char], pairs: &[(char, char)]) -> Vec<(usize, usize)> {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut quote = false;
    let mut escaped = false;
    for (i, c) in text.iter().copied().enumerate() {
        if quote {
            // Inside a string only the closing quote matters, and a
            // backslash takes the next character out of the running.
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quote = false;
            }
            continue;
        }
        if c == '"' {
            quote = true;
        } else if pairs.iter().any(|(open, _)| *open == c) {
            stack.push((c, i));
        } else if let Some((open, _)) = pairs.iter().find(|(_, close)| *close == c)
            && let Some(index) = stack.iter().rposition(|(candidate, _)| *candidate == *open)
        {
            let (_, start) = stack.remove(index);
            out.push((start, i + 1));
        }
    }
    // Outermost first, so that the smallest containing pair is the last one.
    out.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    out
}

/// The string `text[start..end]` is `"..."` in, if it is: (start of the
/// contents, end of the contents), with the quotes just outside.
fn string_in(text: &[char], at: usize) -> Option<(usize, usize)> {
    let quote_at = |from: usize, step: isize| -> Option<usize> {
        let mut i = from as isize;
        let mut escaped = false;
        loop {
            let c = *text.get(i as usize)?;
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                return Some(i as usize);
            }
            i += step;
        }
    };
    // Is point inside a string? Walk from the start of the line, so that a
    // `"` earlier on the same line is what decides.
    let line_start = text[..at.min(text.len())].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
    let mut escaped = false;
    let mut open = None;
    for (i, c) in text[line_start..at.min(text.len())].iter().copied().enumerate() {
        let i = line_start + i;
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            open = open.xor(Some(i));
        }
    }
    let open = open?;
    let close = quote_at(open + 1, 1)?;
    Some((open + 1, close))
}

/// The innermost pair that contains `at`, as (start, end of the whole pair).
fn pair_at(text: &[char], at: usize) -> Option<(usize, usize)> {
    pairs_in(text).into_iter().filter(|(s, e)| *s <= at && at < *e).min_by_key(|(s, e)| e - s)
}

/// The innermost pair that starts at or after `from`.
fn pair_from(text: &[char], from: usize) -> Option<(usize, usize)> {
    pairs_in(text).into_iter().find(|(s, _)| *s >= from)
}

/// A "form": a bracketed pair, or a run of word characters. (start, end).
/// `to` bounds the run so that a word stops at a delimiter.
fn form_bounds(text: &[char], from: usize, to: usize) -> Option<(usize, usize)> {
    let first = *text.get(from)?;
    if PAIRS.iter().any(|(open, _)| *open == first) {
        // Only if the pair closes before `to`: otherwise the enclosing pair
        // would be handed back instead of whatever is inside it.
        return pair_at(text, from).filter(|(_, end)| *end <= to);
    }
    if !is_symbol_or_word(first) {
        return None;
    }
    let limit = to.min(text.len());
    let mut end = from;
    while end < limit && is_symbol_or_word(text[end]) {
        end += 1;
    }
    (end > from).then_some((from, end))
}

/// The first form at or after `at`.
fn form_after(text: &[char], at: usize) -> Option<(usize, usize)> {
    let mut i = at.min(text.len());
    while i < text.len() && text[i].is_whitespace() {
        i += 1;
    }
    form_bounds(text, i, text.len())
}

/// The last form ending at or before `at`.
fn form_before(text: &[char], at: usize) -> Option<(usize, usize)> {
    let mut i = at.min(text.len());
    while i > 0 && text[i - 1].is_whitespace() {
        i -= 1;
    }
    let last = *text.get(i.checked_sub(1)?)?;
    if PAIRS.iter().any(|(_, close)| *close == last) {
        // A pair ending right here: the whole pair.
        return pairs_in(text).into_iter().find(|(_, end)| *end == i);
    }
    if !is_symbol_or_word(last) {
        return None;
    }
    let mut start = i;
    while start > 0 && is_symbol_or_word(text[start - 1]) {
        start -= 1;
    }
    Some((start, i))
}

/// The last form between the brackets at `open` and `close` (both offsets of
/// the brackets themselves): a pair, or a run of word characters.
fn form_inside_before(text: &[char], open: usize, close: usize) -> Option<(usize, usize)> {
    let mut end = close;
    while end > open + 1 && text[end - 1].is_whitespace() {
        end -= 1;
    }
    if end <= open + 1 {
        return None;
    }
    let start = if PAIRS.iter().any(|(open, _)| *open == text[end - 1]) {
        let (start, _) = pairs_in(text).into_iter().find(|(_, e)| *e == end && *e > open + 1)?;
        start
    } else {
        let mut start = end;
        while start > open + 1 && is_symbol_or_word(text[start - 1]) {
            start -= 1;
        }
        start
    };
    (end > open + 1).then_some((start, end))
}

/// The first form between the brackets at `open` and `close`.
fn form_inside_after(text: &[char], open: usize, close: usize) -> Option<(usize, usize)> {
    let mut start = open + 1;
    while start < close && text[start].is_whitespace() {
        start += 1;
    }
    if start >= close {
        return None;
    }
    let end = if PAIRS.iter().any(|(open, _)| *open == text[start]) {
        let (_, end) = pairs_in(text).into_iter().find(|(s, e)| *s == start && *e <= close)?;
        end
    } else {
        let mut end = start;
        while end < close && is_symbol_or_word(text[end]) {
            end += 1;
        }
        end
    };
    (end < close).then_some((start, end))
}

/// The whitespace run at `at`, going forward, as (start, end).
fn space_from(text: &[char], at: usize) -> (usize, usize) {
    let mut end = at;
    while end < text.len() && text[end].is_whitespace() {
        end += 1;
    }
    (at, end)
}

/// The whitespace run ending at `at`, as (start, end).
fn space_before(text: &[char], at: usize) -> (usize, usize) {
    let mut start = at;
    while start > 0 && text[start - 1].is_whitespace() {
        start -= 1;
    }
    (start, at)
}

/// Meow's thing table: which thing each key names.
const THINGS: &[(char, &str)] = &[
    ('r', "round"),
    ('s', "square"),
    ('c', "curly"),
    ('g', "string"),
    ('e', "symbol"),
    ('w', "window"),
    ('b', "buffer"),
    ('p', "paragraph"),
    ('l', "line"),
    ('v', "visual-line"),
    ('d', "defun"),
    ('.', "sentence"),
];

/// The thing table, as the user has it: `meow-char-thing-table` decides
/// which key names which thing. The layout may have its own table, which
/// the user's own table overrides.
pub fn things() -> Vec<(char, String)> {
    let user_set = config::with(|o| o.char_thing_table_is_set);
    if user_set {
        return config::with(|o| {
            o.char_thing_table
                .iter()
                .map(|(k, n)| (*k, n.clone()))
                .collect()
        });
    }
    let name = LAYOUT.with(Cell::get);
    if let Some(layout) = layout::layout(name) {
        if !layout.things.is_empty() {
            return layout
                .things
                .iter()
                .map(|(k, n)| (**k, (*n).to_string()))
                .collect();
        }
    }
    THINGS.iter().map(|(k, n)| (*k, n.to_string())).collect()
}

/// The name of the thing `key` stands for, for the prompt.
pub fn thing_name(key: char) -> Option<String> {
    things()
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, name)| name)
}

/// The thing keys, for a prompt.
pub fn thing_keys() -> String {
    things().into_iter().map(|(k, _)| k).collect()
}

/// The next thing of the same kind after point, as offsets. Bracketed things
/// step one pair at a time; the rest step a run of words.
pub fn next_thing(text: &[char], at: usize, thing: &str) -> Option<(usize, usize)> {
    let bracket = matches!(thing, "round" | "square" | "curly" | "angle");
    if bracket {
        let (wanted, pairs) = match thing {
            "round" => (('(', ')'), &PAIRS[..]),
            "square" => (('[', ']'), &PAIRS[..]),
            "curly" => (('{', '}'), &PAIRS[..]),
            _ => (ANGLE, &[ANGLE][..]),
        };
        return pairs_in_with(text, pairs)
            .into_iter()
            .find(|(s, e)| *s >= at && text.get(*s) == Some(&wanted.0) && text.get(e - 1) == Some(&wanted.1));
    }
    let mut i = at;
    while i < text.len() {
        if let Some((s, e)) = form_bounds(text, i, text.len()) {
            return Some((s, e));
        }
        i += 1;
    }
    None
}

/// The syntax classes a next-thing selection takes in, as
/// `meow-next-thing-include-syntax` gives them: the forward ones, or the
/// backward ones.
fn include_syntax(thing: &str, forward: bool) -> Option<String> {
    config::with(|o| {
        o.thing_syntax(thing)
            .map(|(f, b)| if forward { f.to_string() } else { b.to_string() })
    })
}

/// Whether `c` is one of the syntax classes `syntax` names. Forge's buffers
/// have no syntax table, so the classes are the ones Emacs's standard table
/// uses.
fn syntax_covers(syntax: &str, c: char) -> bool {
    syntax.chars().any(|class| match class {
        '_' => c.is_whitespace(),
        'w' => is_word(c),
        '.' => c.is_ascii_punctuation() && !is_word(c),
        '(' => matches!(c, '(' | '[' | '{'),
        ')' => matches!(c, ')' | ']' | '}'),
        '$' => matches!(c, '"' | '\''),
        '\\' => c == '\\',
        _ => false,
    })
}

/// The thing's selection, with the syntax classes taken into it, as Meow's
/// `meow--fix-thing-selection-mark` does: a forward selection reaches back
/// over them, a backward one reaches on.
fn thing_span_with_syntax(
    text: &[char],
    start: usize,
    end: usize,
    backward: bool,
    syntax: &str,
) -> (usize, usize) {
    if backward {
        let mut e = end;
        while e < text.len() && syntax_covers(syntax, text[e]) {
            e += 1;
        }
        (start, e)
    } else {
        let mut s = start;
        while s > 0 && syntax_covers(syntax, text[s - 1]) {
            s -= 1;
        }
        (s, end)
    }
}

/// The prompt Meow asks for when a command needs a thing.
pub fn thing_prompt(what: &str) {
    let keys = thing_keys();
    read_key(&format!("{what} ({keys}): "), |_| {});
}

/// The text of that prompt. With `meow-display-thing-help` on (the
/// default) the prompt says what the things are, so that the reader does
/// not have to know the keys by heart.
pub fn thing_prompt_text(what: &str) -> String {
    if config::with(|o| o.display_thing_help) {
        let list: Vec<String> = things()
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect();
        format!("{what} ({}): ", list.join(", "))
    } else {
        format!("{what} ({}): ", thing_keys())
    }
}

/// The text of the thing `thing` covers at point, as offsets into the flat
/// text: (start, end). `inner` leaves the delimiters out.
fn thing_span(b: &Buffer, thing: &str, inner: bool) -> Option<(usize, usize)> {
    let text = flat_text(b);
    let at = offset_of(b, b.point());
    // The innermost pair at point that is the right shape; failing that,
    // whichever pair is there, so that `s` on a curly still does something
    // sensible.
    let brackets = |open: char, close: char| -> Option<(usize, usize)> {
        let all = || pairs_in(&text).into_iter().filter(|(s, e)| *s <= at && at < *e);
        let (s, e) = all()
            .find(|(s, e)| text.get(*s) == Some(&open) && text.get(e - 1) == Some(&close))
            .or_else(|| all().next())?;
        Some(if inner { (s + 1, e - 1) } else { (s, e) })
    };
    Some(match thing {
        "round" => brackets('(', ')')?,
        "square" => brackets('[', ']')?,
        "curly" => brackets('{', '}')?,
        "string" => {
            let (s, e) = string_in(&text, at)?;
            if inner { (s, e) } else { (s - 1, e + 1) }
        }
        "symbol" => {
            let (s, e) = symbol_bounds(&b.line(b.point().line), b.point().column as usize);
            let line = offset_of(b, Position { line: b.point().line, column: 0 });
            (line + s, line + e)
        }
        "line" | "visual-line" => {
            // Both are the line; a visual line is the same when lines aren't
            // wrapped, and this is close enough when they are.
            let line = b.point().line as usize;
            let start = offset_of(b, Position { line: line as u32, column: 0 });
            let end = start + b.line(line as u32).chars().count();
            if inner {
                let indent = text[start..end].iter().take_while(|c| c.is_whitespace()).count();
                (start + indent, end)
            } else {
                (start, end)
            }
        }
        "buffer" | "window" => {
            let visible = visible_span(b);
            if inner { visible } else { (0, text.len()) }
        }
        "paragraph" | "defun" | "sentence" => {
            let start = blank_line_before(&text, at);
            let end = blank_line_after(&text, at);
            (start, end)
        }
        _ => return None,
    })
}

/// The part of the buffer "on screen", as flat-text offsets. A plugin can't
/// ask which lines a window shows, so this is all of it.
fn visible_span(b: &Buffer) -> (usize, usize) {
    (0, flat_text(b).len())
}

/// Where the paragraph (or defun, or sentence) at `at` begins: after the
/// last blank line above it.
fn blank_line_before(text: &[char], at: usize) -> usize {
    let mut start = 0;
    for i in 0..at.min(text.len()) {
        if text.get(i) == Some(&'\n') && text.get(i + 1) == Some(&'\n') {
            start = i + 2;
        }
    }
    start
}

/// ...and where it ends: at the next blank line.
fn blank_line_after(text: &[char], at: usize) -> usize {
    let mut end = text.len();
    for i in at..text.len() {
        if text.get(i) == Some(&'\n') && text.get(i + 1) == Some(&'\n') {
            end = i;
            break;
        }
    }
    end
}

fn thing_bounds(b: &Buffer, thing: &str, inner: bool) -> Option<(Position, Position)> {
    let (start, end) = thing_span(b, thing, inner)?;
    Some((position_at(b, start), position_at(b, end)))
}

fn move_symbol(b: &Buffer, n: i64) {
    let text = flat_text(b);
    let mut offset = offset_of(b, b.point()).min(text.len());
    let forward = n >= 0;
    for _ in 0..n.unsigned_abs().max(1) {
        if forward {
            while offset < text.len() && !is_symbol(text[offset]) {
                offset += 1;
            }
            while offset < text.len() && is_symbol(text[offset]) {
                offset += 1;
            }
        } else {
            while offset > 0 && !is_symbol(text[offset - 1]) {
                offset -= 1;
            }
            while offset > 0 && is_symbol(text[offset - 1]) {
                offset -= 1;
            }
        }
    }
    b.set_point(position_at(b, offset));
}

fn run_n(cmd: &str, n: Option<i64>) -> Result<(), String> {
    for _ in 0..n.unwrap_or(1).max(1) {
        run_command(cmd)?;
    }
    Ok(())
}

/// Move point one character at a time, away from the mark, `count` times.
/// Shared by `meow-expand` and `meow-expand-0` through `-9`, so the digit
/// variants and the counted one can't drift apart.
fn expand(count: i64) {
    let b = cur();
    if b.region().is_none() {
        message("No selection to expand");
        return;
    }
    // Some modes don't want their selections expanded: `meow-expand-exclude-mode-list`.
    let mode = b.major_mode().to_string();
    if config::with(|o| o.expand_exclude_mode_list.iter().any(|m| *m == mode)) {
        message(&format!("Expansion is off in {mode}"));
        return;
    }
    let mark = b.mark().unwrap_or(b.point());
    let forward = offset_of(&b, b.point()) >= offset_of(&b, mark);
    // What the selection is made of: Meow expands by the same thing the
    // selection was made of, so a word selection grows by words.
    let kind = SELECTION_KIND.with(Cell::get);
    let text = flat_text(&b);
    let count = count.max(1) as usize;
    for _ in 0..count {
        let p = b.point();
        let at = offset_of(&b, p);
        let target = match kind {
            "word" => word_step(&text, at, forward).map(|o| position_at(&b, o)),
            "symbol" => symbol_step(&text, at, forward).map(|o| position_at(&b, o)),
            "line" => line_step(&b, p, forward),
            _ => char_step(&b, p, forward),
        };
        if let Some(target) = target {
            b.set_point(target);
        }
    }
    // Meow's `meow-expand-selection-type` says whether the new selection is
    // a plain selection or an expansion of the old one.
    EXPANDED.with(|e| e.set(config::with(|o| o.expand_selection_type == "expand")));
    // Meow says how far the selection could still grow. How many positions
    // it counts is `meow-expand-hint-counts`.
    let hint = expand_hint(&b, kind, forward);
    let what = match kind {
        "word" => "words",
        "symbol" => "symbols",
        "line" => "lines",
        _ => "characters",
    };
    message(&format!("Expand: {hint} more {what}"));
}

/// How many more times the selection could be expanded, which is what
/// Meow's hint counts. The cap is `meow-expand-hint-counts`.
fn expand_hint(b: &Buffer, kind: &str, forward: bool) -> usize {
    let cap = config::with(|o| o.expand_hint_count(kind)) as usize;
    let text = flat_text(b);
    let mut p = b.point();
    let mut at = offset_of(b, p);
    let mut count = 0;
    while count < cap {
        let next = match kind {
            "word" => word_step(&text, at, forward).map(|o| position_at(b, o)),
            "symbol" => symbol_step(&text, at, forward).map(|o| position_at(b, o)),
            "line" => line_step(b, p, forward),
            _ => char_step(b, p, forward),
        };
        match next {
            Some(next) => {
                p = next;
                at = offset_of(b, next);
                count += 1;
            }
            None => break,
        }
    }
    count
}

/// The next word boundary after `at`, or the one before it.
fn word_step(text: &[char], at: usize, forward: bool) -> Option<usize> {
    let mut i = at;
    if forward {
        while i < text.len() && !is_word(text[i]) {
            i += 1;
        }
        while i < text.len() && is_word(text[i]) {
            i += 1;
        }
    } else {
        while i > 0 && !is_word(text[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(text[i - 1]) {
            i -= 1;
        }
    }
    (i != at).then_some(i)
}

/// The next symbol boundary after `at`, or the one before it.
fn symbol_step(text: &[char], at: usize, forward: bool) -> Option<usize> {
    let mut i = at;
    if forward {
        while i < text.len() && !is_symbol(text[i]) {
            i += 1;
        }
        while i < text.len() && is_symbol(text[i]) {
            i += 1;
        }
    } else {
        while i > 0 && !is_symbol(text[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_symbol(text[i - 1]) {
            i -= 1;
        }
    }
    (i != at).then_some(i)
}

/// The next line after `p`, or the one before it: the start of the next
/// line, or the end of the last one before it.
fn line_step(b: &Buffer, p: Position, forward: bool) -> Option<Position> {
    if forward {
        if p.line + 1 < b.line_count() {
            Some(pos(p.line + 1, 0))
        } else {
            None
        }
    } else if p.line > 0 {
        let line = p.line - 1;
        Some(pos(line, char_len(&b.line(line))))
    } else {
        None
    }
}

/// The next character after `p`, or the one before it.
fn char_step(b: &Buffer, p: Position, forward: bool) -> Option<Position> {
    if forward {
        if p.column < char_len(&b.line(p.line)) {
            Some(pos(p.line, p.column + 1))
        } else if p.line + 1 < b.line_count() {
            Some(pos(p.line + 1, 0))
        } else {
            None
        }
    } else if p.column > 0 {
        Some(pos(p.line, p.column - 1))
    } else if p.line > 0 {
        let line = p.line - 1;
        Some(pos(line, char_len(&b.line(line))))
    } else {
        None
    }
}

fn run_signed(forward: &str, backward: &str, n: Option<i64>) -> Result<(), String> {
    let n = n.unwrap_or(1);
    let (cmd, count) = if n < 0 {
        (backward, n.saturating_abs())
    } else {
        (forward, n)
    };
    for _ in 0..count.max(1) {
        run_command(cmd)?;
    }
    Ok(())
}

fn find(include: bool, n: Option<i64>) {
    let prompt = if include { "Find: " } else { "Till: " };
    read_key(prompt, move |key| {
        let Some(key) = key else { return };
        let key = if key == "SPC" { " ".to_string() } else { key };
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return message(&format!("Not a character: {key}"));
        };
        let b = cur();
        let text = flat_text(&b);
        let start = offset_of(&b, b.point());
        let backward = n.unwrap_or(1) < 0;
        let count = n.unwrap_or(1).unsigned_abs().max(1) as usize;
        LAST_CHAR.with(|last| last.set(Some(c)));
        match find_target(&text, start, c, count, backward, include) {
            Some(target) => {
                select_from_point(&b);
                b.set_point(position_at(&b, target));
            }
            None => message(&format!("No {c} {} point", if backward { "before" } else { "after" })),
        }
    });
}

// ------------------------------------------------------- structural edits

/// A change to the flat text: replace `span` with `text`, and leave point
/// at `point` (an offset into the text *after* the change, or None to leave
/// it be). Every op is planned against the text as it is now, so that the
/// planning half can be tested without a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    span: (usize, usize),
    text: String,
    point: Option<usize>,
}

impl Plan {
    fn replace(span: (usize, usize), text: String) -> Plan {
        Plan { span, text, point: None }
    }

    /// The same, leaving point just after what was put in.
    fn replace_leaving(span: (usize, usize), text: String, at: usize) -> Plan {
        Plan { span, text, point: Some(at) }
    }
}

/// Whether a character can be part of a bare word or symbol.
fn is_symbol_or_word(c: char) -> bool {
    is_symbol(c) || is_word(c)
}

/// The text between two flat offsets.
fn slice(text: &[char], start: usize, end: usize) -> String {
    text.get(start..end.min(text.len()))
        .unwrap_or_default()
        .iter()
        .collect()
}

/// Slurp: pull the next (or previous) form into the pair at point.
/// `(a b) c` -> `(a b c)`, and `c (a b)` -> `(c a b)`.
pub fn plan_slurp(text: &[char], at: usize, forward: bool) -> Option<Plan> {
    let (open, close) = pair_at(text, at)?;
    let close_char = close - 1;
    let closing = text.get(close_char).copied().unwrap_or(')');
    let opening = text.get(open).copied().unwrap_or('(');
    if forward {
        // The form after the pair moves in before its bracket, and the gap
        // between them goes with it.
        let (start, end) = form_after(text, close)?;
        let gap = slice(text, close, start);
        Some(Plan::replace((close_char, end), format!("{gap}{}{closing}", slice(text, start, end))))
    } else {
        // The form before the pair moves in after its bracket. It is the
        // one before the pair's own bracket, not before point.
        let (start, end) = form_before(text, open)?;
        let inner = slice(text, open + 1, close_char);
        // `c (a b)` -> `(c a b)`; a gap inside the pair is kept as it is.
        let sep = if inner.is_empty() || inner.starts_with(char::is_whitespace) { "" } else { " " };
        let body = slice(text, start, end);
        Some(Plan::replace((start, close), format!("{opening}{body}{sep}{inner}{closing}")))
    }
}

/// Barf: push the last (or first) form out of the pair at point.
/// `(a b c) d` -> `(a b) c d`, and `(a b c) d` -> `a (b c) d`.
pub fn plan_barf(text: &[char], at: usize, forward: bool) -> Option<Plan> {
    let (open, close) = pair_at(text, at)?;
    let close_char = close - 1;
    let closing = text.get(close_char).copied().unwrap_or(')');
    let opening = text.get(open).copied().unwrap_or('(');
    if forward {
        // The last form inside comes out after the pair, taking the space it
        // had with it: `(a b c) d` -> `(a b) c d`.
        let (start, end) = form_inside_before(text, open, close_char)?;
        // With only one form inside, barf would leave an empty pair.
        if start <= open + 1 {
            return None;
        }
        let (gap_start, _) = space_before(text, start);
        let (_, after) = space_from(text, close);
        let gap_out = slice(text, close, after);
        let gap_in = slice(text, gap_start, start);
        let body = slice(text, start, end);
        // The space the form had inside comes out with it.
        Some(Plan::replace((gap_start, after), format!("{closing}{gap_out}{body}{gap_in}")))
    } else {
        // The first form inside comes out before the pair: `(a b c) d` ->
        // `a (b c) d`.
        let (start, end) = form_inside_after(text, open, close_char)?;
        if end >= close_char {
            return None;
        }
        let (_, gap_end) = space_from(text, end);
        // The gap the form left is consumed with it, so the pair closes up
        // around whatever the form was followed by.
        let (_, inner_end) = space_from(text, open + 1);
        let inner_gap = slice(text, open + 1, inner_end);
        let sep = match inner_gap.is_empty() { true => " ".to_string(), false => inner_gap };
        Some(Plan::replace((open, gap_end), format!("{}{sep}{opening}", slice(text, start, end))))
    }
}

/// Raise: lift the pair at point out into the place of the one around it,
/// so `(a (b c))` becomes `(b c)`. Nothing happens when there is no pair
/// above to replace.
pub fn plan_raise(text: &[char], at: usize) -> Option<Plan> {
    let inner = pair_at(text, at)?;
    let outer = pairs_in(text)
        .into_iter()
        .find(|(s, e)| *s <= inner.0 && inner.1 <= *e)?;
    (outer.0 != inner.0).then(|| {
        let body = slice(text, inner.0, inner.1);
        Plan::replace_leaving(outer, body, inner.0)
    })
}

/// Splice: remove the brackets of the pair at point, keeping the contents,
/// so `(a (b c))` becomes `(a b c)`.
pub fn plan_splice(text: &[char], at: usize) -> Option<Plan> {
    let (open, close) = pair_at(text, at)?;
    let inner = slice(text, open + 1, close - 1);
    Some(Plan::replace_leaving((open, close), inner, open))
}

/// Transpose: swap the two forms either side of point.
pub fn plan_transpose(text: &[char], at: usize) -> Option<Plan> {
    let left = form_before(text, at);
    let right = form_after(text, at);
    let (Some((ls, le)), Some((rs, re))) = (left, right) else {
        return None;
    };
    if le > rs {
        return None;
    }
    let before = slice(text, ls, le);
    let after = slice(text, rs, re);
    let gap = slice(text, le, rs);
    let point = ls + after.chars().count() + gap.chars().count();
    Some(Plan::replace_leaving((ls, re), format!("{after}{gap}{before}"), point))
}

/// Split: a space at point, so a bracket or word opens up around it.
pub fn plan_split(_text: &[char], at: usize) -> Plan {
    Plan::replace((at, at), " ".to_string())
}

/// Join: take the whitespace at point out of the pair around it, so
/// `(a | b)` becomes `(a b)`.
pub fn plan_join(text: &[char], at: usize) -> Option<Plan> {
    let (open, close) = pair_at(text, at)?;
    let (gap_start, gap_end) = space_from(text, at);
    // Point has to be on whitespace inside a pair: the gap between two
    // pairs, or a point on a bracket, has nothing to join.
    if gap_end == gap_start || gap_start < open + 1 || gap_end > close - 1 {
        return None;
    }
    // The two forms meet: whatever whitespace was between them goes.
    Some(Plan::replace_leaving((gap_start, gap_end), String::new(), gap_start))
}

/// Wrap: put the selection, or the form at point, inside a pair.
pub fn plan_wrap(text: &[char], span: (usize, usize), open: char, close: char) -> Plan {
    let body = slice(text, span.0, span.1);
    let point = span.0 + 1 + body.chars().count();
    Plan::replace_leaving(span, format!("{open}{body}{close}"), point)
}

/// The form point is in or beside, as offsets: a pair when it is inside one,
/// otherwise the nearest word or pair.
pub fn form_at(text: &[char], at: usize) -> Option<(usize, usize)> {
    let under = |i: usize| text.get(i).is_some_and(|c| is_symbol_or_word(*c));
    if under(at) || (at > 0 && under(at - 1)) {
        let mut start = if under(at) { at } else { at - 1 };
        while start > 0 && under(start - 1) {
            start -= 1;
        }
        let mut end = at.max(start);
        while under(end) {
            end += 1;
        }
        return Some((start, end));
    }
    pair_at(text, at).or_else(|| form_after(text, at))
}

/// Carry out a [`Plan`] on the buffer.
fn apply(b: &Buffer, plan: &Plan) {
    let at = position_at(b, plan.span.0);
    let end = position_at(b, plan.span.1);
    b.replace(Span { start: at, end }, &plan.text);
    if let Some(point) = plan.point {
        b.set_point(position_at(b, point));
    }
}

/// Plan against the buffer's own text, and carry the plan out.
fn run_plan(b: &Buffer, plan: Option<Plan>, when_missing: &str) {
    match plan {
        Some(plan) => apply(b, &plan),
        None => message(when_missing),
    }
}

// ------------------------------------------------------------- keypad

/// What the keypad makes of one key: the sequence so far, whether the
/// next key is literal (after SPC), a pending modifier (m, g).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keypad {
    /// Read another key with this state.
    More {
        seq: Vec<String>,
        literal: bool,
        modifier: Option<&'static str>,
    },
    /// Look up this sequence (and `plain`, if the Control version isn't
    /// bound: C-x f rather than C-x C-f).
    Try {
        seq: Vec<String>,
        plain: Option<Vec<String>>,
    },
}

/// The key a character is written as in a key description: a space is
/// `SPC`, anything else is itself.
fn key_name(c: char) -> String {
    if c == ' ' { "SPC".to_string() } else { c.to_string() }
}

/// The keypad's step for `key` (a key description) in the given state.
pub fn keypad_step(
    mut seq: Vec<String>,
    literal: bool,
    modifier: Option<&'static str>,
    key: &str,
) -> Keypad {
    config::with(|o| {
        let literal_key = key_name(o.keypad_literal_prefix);
        if key == literal_key && !literal && modifier.is_none() && !seq.is_empty() {
            return Keypad::More {
                seq,
                literal: true,
                modifier: None,
            };
        }
        if seq.is_empty() && modifier.is_none() {
            // A key that starts a Control sequence: c, h and x by default.
            if let Some(c) = key.chars().next()
                && key.chars().count() == 1
                && let Some(to) = o.keypad_start(c)
            {
                seq.push(format!("C-{to}"));
                return Keypad::Try { seq, plain: None };
            }
            if key.chars().count() == 1 {
                let meta = key_name(o.keypad_meta_prefix);
                let ctrl_meta = key_name(o.keypad_ctrl_meta_prefix);
                if key == meta {
                    return Keypad::More {
                        seq,
                        literal: false,
                        modifier: Some("M-"),
                    };
                }
                if key == ctrl_meta {
                    return Keypad::More {
                        seq,
                        literal: false,
                        modifier: Some("C-M-"),
                    };
                }
            }
            seq.push("C-c".into());
            seq.push(key.into());
            return Keypad::Try { seq, plain: None };
        }
        if let Some(m) = modifier {
            seq.push(format!("{m}{key}"));
            return Keypad::Try { seq, plain: None };
        }
        if literal || key.starts_with("C-") || key.starts_with("M-") || key.chars().count() > 1 {
            seq.push(key.into());
            return Keypad::Try { seq, plain: None };
        }
    let mut plain = seq.clone();
    plain.push(key.into());
    seq.push(format!("C-{key}"));
        Keypad::Try {
            seq,
            plain: Some(plain),
        }
    })
}

/// Read digits, then one more key, and run that key's binding with the
/// digits as its count: `3` then `d` deletes three characters. This is
/// Meow's leader layer; Forge's own `C-c` map is the other way in.
fn digit_argument() {
    let b = cur();
    // Show that a count is being typed, so the lighter says why the keys
    // being read are not doing anything yet.
    let _ = b.enable_minor_mode(LEADER);
    read_key("Argument: ", move |first| {
        let Some(first) = first else { return leave_leader() };
        let Some(digit) = first.chars().next().filter(char::is_ascii_digit) else {
            leave_leader();
            return message(&format!("Not a digit: {first}"));
        };
        digits(0, digit.to_digit(10).unwrap_or(0) as i64);
    });
}

/// Back out of the leader layer, leaving normal state as it found things.
fn leave_leader() {
    let b = cur();
    b.disable_minor_mode(LEADER);
    if !b.minor_modes().contains(&NORMAL.to_string()) {
        normal(&b);
    }
}

/// Keep reading digits, then take the next key as the command.
fn digits(count: i64, next: i64) {
    let count = count * 10 + next;
    read_key(&format!("({count}) "), move |key| {
        let Some(key) = key else { return leave_leader() };
        let c = key.chars().next().unwrap_or(' ');
        if c.is_ascii_digit() {
            return digits(count, c.to_digit(10).unwrap_or(0) as i64);
        }
        // The key has to be looked up in normal state, not the leader's.
        leave_leader();
        // A prefix argument outlives the key that set it, so the count
        // reaches the command the user was really after.
        let _ = execute_keys(&format!("C-u {count} {key}"));
    });
}

/// Read one key and say what it is bound to.
fn describe_key() {
    read_key("Describe key: ", |key| {
        let Some(key) = key else { return };
        match keymap::lookup(&key) {
            Some(command) => message(&format!("{key} runs {command}")),
            None if keymap::is_prefix(&key) => message(&format!("{key} is a prefix")),
            None => message(&format!("{key} is undefined")),
        }
    });
}

/// A message from the keypad, with the keypad's prefix, unless the user
/// has turned the keypad's messages off.
fn keypad_says(text: &str) {
    config::with(|o| {
        if o.keypad_message {
            message(&format!("{}{text}", o.keypad_message_prefix));
        }
    });
}

fn keypad(seq: Vec<String>, literal: bool, modifier: Option<&'static str>) {
    extra::remember_keypad(&seq);
    let shown = config::with(|o| {
        if !o.keypad_message {
            return String::new();
        }
        format!(
            "{}SPC {}{}",
            o.keypad_message_prefix,
            seq.join(" "),
            modifier.map(|m| format!(" {m}")).unwrap_or_default()
        )
    });
    read_key(&shown, move |key| {
        let Some(key) = key else {
            return quit_keypad();
        };
        if key == "DEL" || key == "<backspace>" {
            let mut seq = seq;
            seq.pop();
            if seq.is_empty() {
                quit_keypad();
                let _ = open_keypad();
            } else {
                keypad(seq, false, None);
            }
            return;
        }
        match keypad_step(seq, literal, modifier, &key) {
            Keypad::More {
                seq,
                literal,
                modifier,
            } => keypad(seq, literal, modifier),
            Keypad::Try { seq, plain } => {
                let bound = |s: &[String]| {
                    let keys = s.join(" ");
                    (keymap::lookup(&keys).is_some(), keymap::is_prefix(&keys))
                };
                let mut seq = seq;
                if let Some(p) = plain
                    && bound(&seq) == (false, false) {
                    seq = p;
                    }
                match bound(&seq) {
                    (true, _) => {
                        let _ = execute_keys(&seq.join(" "));
                        quit_keypad();
                    }
                    (false, true) => keypad(seq, false, None),
                    _ => {
                        let keys = seq.join(" ");
                        quit_keypad();
                        keypad_says(&format!("{keys} is undefined"));
                    }
                }
            }
        }
    });
}

/// The transient map's name. While it is up, its keys outrank every other
/// layer, and `where-is`, `describe-key` and the prefix hints see it.
const KEYPAD: &str = "meow-keypad";

/// The keys the keypad offers at its first step. Every printable key is
/// offered, because the keypad's rule is "SPC <key> is C-c <key>"; the
/// handful of named keys it knows are listed with the printable ones.
fn keypad_keys() -> Vec<String> {
    let mut keys: Vec<String> = (' '..='~')
        .map(|c| match c {
            ' ' => "SPC".to_string(),
            c => c.to_string(),
        })
        .collect();
    for key in ["RET", "TAB", "DEL", "C-x"] {
        keys.push(key.to_string());
    }
    keys
}

/// One command per keypad key. Command names may only be alphanumeric and
/// a few punctuation marks, and the keypad's keys include `"` and `[`, so
/// the keys are numbered rather than spelled out.
fn keypad_command(n: usize) -> String {
    format!("meow-keypad-k{n}")
}

/// One key of the keypad, read from the transient map. It is the first key
/// of a sequence: the rest are read by [`keypad`] itself, so nothing is
/// carried over from one sequence to the next.
fn keypad_press(key: &str) -> Result<(), String> {
    match keypad_step(Vec::new(), false, None, key) {
        Keypad::More {
            seq,
            literal,
            modifier,
        } => {
            // The rest of the sequence is read directly: leaving the map up
            // would make it swallow the key before it is read.
            keypad(seq, literal, modifier);
            Ok(())
        }
        Keypad::Try { seq, plain } => {
            // The sequence is complete, so the map has done its job. Taking
            // it down first also stops it from answering the lookup below:
            // the keys have to be resolved in the layers underneath.
            quit_keypad();
            let bound = |s: &[String]| {
                let keys = s.join(" ");
                (keymap::lookup(&keys).is_some(), keymap::is_prefix(&keys))
            };
            let mut seq = seq;
            if let Some(p) = plain
                && bound(&seq) == (false, false)
            {
                seq = p;
            }
            match bound(&seq) {
                (true, _) => {
                    let _ = execute_keys(&seq.join(" "));
                    Ok(())
                }
                (false, true) => {
                    keypad(seq, false, None);
                    Ok(())
                }
                _ => {
                    message(&format!("{} is undefined", seq.join(" ")));
                    Ok(())
                }
            }
        }
    }
}

/// Close the keypad, whether or not it is up. Safe to call twice.
fn quit_keypad() {
    let _ = pop_transient(KEYPAD);
}

/// Bind the keypad's own keys into the transient map, then put it up. The
/// map is what the keys are actually read from, so SPC x f is a real key
/// sequence rather than keys read one at a time.
fn open_keypad() -> Result<(), String> {
    // The map is filled in on every open, so a stale binding from last time
    // can't outlive it.
    for (n, key) in keypad_keys().into_iter().enumerate() {
        bind_transient(KEYPAD, &key, &keypad_command(n))?;
    }
    bind_transient(KEYPAD, "ESC", "meow-keypad-quit")?;
    bind_transient(KEYPAD, "C-g", "meow-keypad-quit")?;
    // Stay, not Exit: a key the map doesn't know is still the user's, and
    // the map is closed by the command that runs instead.
    push_transient(KEYPAD, TransientOpts::default().on_unbound("stay"))
}

// --------------------------------------------------------------- layers

/// The keys the plugin adds to every layout: the keypad, and the
/// extended command. Meow leaves these to the reader, so they are the
/// plugin's own.
const LAYER_EXTRAS: &[(&str, &str)] = &[(":", "execute-extended-command"), ("SPC", "meow-keypad")];

/// Put a layout's keys into the normal state, and say which layout it is.
///
/// The keys the editor has no room for (a layout that names a key with a
/// character the editor cannot bind) are left out, and said in the
/// message, rather than stopping the switch half-done.
fn setup_layout(name: &str) -> Result<(), String> {
    let Some(layout) = layout::layout(name) else {
        return Err(format!("No Meow layout called {name}"));
    };
    let keys = layout::normal_keys(layout);
    let mut skipped = bind_all(&keys);
    for (key, command) in LAYER_EXTRAS {
        if let Err(e) = bind_in_mode(NORMAL, key, command) {
            skipped.push(format!("{key} ({e})"));
        }
    }
    for (key, command) in layout.motion {
        if let Err(e) = bind_motion(key, command) {
            skipped.push(format!("{key} ({e})"));
        }
    }
    // Meow leaves the escape key alone in normal state; the plugin makes it
    // cancel the selection, which is what a reader expects of it.
    if let Err(e) = bind_in_mode(NORMAL, "ESC", "meow-cancel-selection") {
        skipped.push(format!("ESC ({e})"));
    }
    LAYOUT.with(|l| l.set(layout.name));
    let mut said = format!("Meow layout: {}", layout.name);
    if !skipped.is_empty() {
        said.push_str(&format!(" (left out: {})", skipped.join(", ")));
    }
    message(&said);
    Ok(())
}

/// Bind a layer into the normal-state keymap, and point every printable key
/// the layer says nothing about at "this does nothing": in normal state a
/// key must never insert. The keys the editor will not take are collected
/// rather than stopping the layer half-bound.
fn bind_all(layer: &[(&str, &str)]) -> Vec<String> {
    let mut skipped = Vec::new();
    for (key, command) in layer {
        if let Err(e) = bind_in_mode(NORMAL, key, command) {
            skipped.push(format!("{key} ({e})"));
        }
    }
    for c in '!'..='~' {
        let key = c.to_string();
        if !layer.iter().any(|(k, _)| *k == key) {
            let _ = bind_in_mode(NORMAL, &key, "meow-undefined");
        }
    }
    skipped
}

// ----------------------------------------------------------- commands

fn init() -> Result<(), String> {
    config::load();
    modes::define_minor_mode(NORMAL)?;
    modes::define_minor_mode(INSERT)?;
    modes::define_minor_mode(LEADER)?;
    modes::define_minor_mode(MOTION)?;
    modes::set_lighter(NORMAL, "[N]")?;
    modes::set_lighter(INSERT, "[I]")?;
    modes::set_lighter(LEADER, "[#]")?;
    modes::set_lighter(MOTION, "[M]")?;

    for (key, name, builtin) in [
        ("h", "meow-left", "backward-char"),
        ("j", "meow-next", "next-line"),
        ("k", "meow-prev", "previous-line"),
        ("l", "meow-right", "forward-char"),
    ] {
        command(
            name,
            &format!("Move ({builtin}), cancelling the selection."),
            move |n| {
                // Meow keeps an expanded character selection, so moving
                // through it doesn't throw it away.
                if !expanded_char() {
                    cancel(&cur());
                }
                run_n(builtin, n)
            },
        )?;
        bind_in_mode(NORMAL, key, name)?;
    }

    for (name, builtin, doc) in [
        ("meow-left-expand", "backward-char", "Extend the selection one character left."),
        ("meow-right-expand", "forward-char", "Extend the selection one character right."),
        ("meow-prev-expand", "previous-line", "Extend the selection to the previous line."),
        ("meow-next-expand", "next-line", "Extend the selection to the next line."),
    ] {
        command(name, doc, move |n| {
            let b = cur();
            select_from_point(&b);
            run_n(builtin, n)?;
            // Meow's expand commands make an '(expand . char) selection.
            SELECTION_KIND.with(|k| k.set("char"));
            EXPANDED.with(|e| e.set(true));
            Ok(())
        })?;
    }
    for (name, forward, backward, doc) in [
        ("meow-page-up", "scroll-down-command", "scroll-up-command", "Scroll up by a page."),
        ("meow-page-down", "scroll-up-command", "scroll-down-command", "Scroll down by a page."),
    ] {
        command(name, doc, move |n| {
            cancel(&cur());
            run_signed(forward, backward, n)
        })?;
    }

    command("meow-mark-word", "Select the word at point.", |n| {
        let b = cur();
        let p = b.point();
        let (s, e) = word_bounds(&b.line(p.line), p.column as usize);
        select_oriented_kind(
            &b,
            pos(p.line, s as u32),
            pos(p.line, e as u32),
            n.unwrap_or(1) < 0,
            "word",
        );
        Ok(())
    })?;
    command(
        "meow-next-word",
        "Select to the end of the next word (extending a selection).",
        |n| {
            select_from_point(&cur());
            run_signed("forward-word", "backward-word", n)?;
            SELECTION_KIND.with(|k| k.set("word"));
            Ok(())
        },
    )?;
    command(
        "meow-back-word",
        "Select to the start of the previous word (extending a selection).",
        |n| {
            select_from_point(&cur());
            run_signed("backward-word", "forward-word", n)?;
            SELECTION_KIND.with(|k| k.set("word"));
            Ok(())
        },
    )?;
    command("meow-mark-symbol", "Select the symbol at point.", |n| {
        let b = cur();
        let p = b.point();
        let (s, e) = symbol_bounds(&b.line(p.line), p.column as usize);
        select_oriented_kind(
            &b,
            pos(p.line, s as u32),
            pos(p.line, e as u32),
            n.unwrap_or(1) < 0,
            "symbol",
        );
        Ok(())
    })?;
    command(
        "meow-next-symbol",
        "Select to the end of the next symbol (extending a selection).",
        |n| {
            let b = cur();
            select_from_point(&b);
            move_symbol(&b, n.unwrap_or(1));
            SELECTION_KIND.with(|k| k.set("symbol"));
            Ok(())
        },
    )?;
    command(
        "meow-back-symbol",
        "Select to the start of the previous symbol (extending a selection).",
        |n| {
            let b = cur();
            select_from_point(&b);
            move_symbol(&b, -n.unwrap_or(1));
            SELECTION_KIND.with(|k| k.set("symbol"));
            Ok(())
        },
    )?;
    command("meow-line", "Select the line; again, one more line.", |n| {
        let b = cur();
        let n = n.unwrap_or(1).max(1) as u32;
        let last_line = b.line_count().saturating_sub(1);
        // Extend only a selection this command made: a word that happens to
        // start at column 0 is not whole lines.
        let again = match (b.region(), LINE_SEL.with(Cell::get)) {
            (Some(r), Some((id, s, e))) if id == b.id() && same(r.start, s) && same(r.end, e) => Some(r),
            _ => None,
        };
        match again {
            Some(Span { end, .. }) => {
                let last = (end.line + n).min(last_line);
                b.set_point(pos(last, char_len(&b.line(last))));
            }
            None => {
                let line = b.point().line;
                b.set_mark(Some(pos(line, 0)));
                let last = (line + n - 1).min(last_line);
                b.set_point(pos(last, char_len(&b.line(last))));
            }
        }
        LINE_SEL.with(|sel| sel.set(b.region().map(|r| (b.id(), r.start, r.end))));
        SELECTION_KIND.with(|k| k.set("line"));
        Ok(())
    })?;
    command(
        "meow-find",
        "Select to the next occurrence of a character (read next), including it.",
        |n| {
            find(true, n);
            Ok(())
        },
    )?;
    command("meow-find-expand", "Find a character while extending the selection.", |n| {
        find(true, n);
        Ok(())
    })?;
    command(
        "meow-till",
        "Select up to the next occurrence of a character (read next).",
        |n| {
            find(false, n);
            Ok(())
        },
    )?;
    command("meow-till-expand", "Till a character while extending the selection.", |n| {
        find(false, n);
        Ok(())
    })?;
    command(
        "meow-reverse",
        "Swap the ends of the selection.\n\nWith no selection this says \"No selection\", as Meow does: there is no fallback for it by default.",
        |_| {
            with_selection_fallback("meow-reverse", || {
                run_command("exchange-point-and-mark")
            })
        },
    )?;
    command(
        "meow-cancel-selection",
        "Cancel the selection.\n\nWith no selection this runs the fallback for meow-cancel-selection, which is keyboard-quit unless you set another.",
        |_| {
            with_selection_fallback("meow-cancel-selection", || {
                cancel(&cur());
                Ok(())
            })
        },
    )?;
    command(
        "meow-kill",
        "Kill the selection.\n\nWith no selection this runs the fallback for meow-kill, which is meow-C-k (kill the line) unless you set another.",
        |_| {
            with_selection_fallback("meow-kill", || {
                let b = cur();
                let Some(region) = b.region() else { return Ok(()) };
                record_kill(b.text_in(region));
                run_command("kill-region")
            })
        },
    )?;
    command(
        "meow-change",
        "Kill the selection and start inserting.\n\nWith no selection this runs the fallback for meow-change, which is meow-change-char unless you set another.",
        |_| {
            with_selection_fallback("meow-change", || {
                let b = cur();
                let Some(region) = b.region() else { return Ok(()) };
                record_kill(b.text_in(region));
                run_command("kill-region")?;
                insert_for(&b, "change");
                Ok(())
            })
        },
    )?;
    command(
        "meow-save",
        "Copy the selection, the way kill-ring-save does.\n\nWith no selection this runs the fallback for meow-save, which is kill-ring-save unless you set another.",
        |_| {
            with_selection_fallback("meow-save", || {
                let b = cur();
                let Some(region) = b.region() else { return Ok(()) };
                record_kill(b.text_in(region));
                run_command("kill-ring-save")
            })
        },
    )?;
    command(
        "meow-yank",
        "Paste (yank).\n\nWith meow-use-clipboard on this takes the editor's own kill ring, so anything killed anywhere in the editor can be pasted.",
        |_| {
            if let Some(text) = yank_text() {
                let b = cur();
                if let Some(region) = b.region() {
                    b.delete(region);
                }
                insert_yank(&b, &text);
                Ok(())
            } else {
                run_command("yank")
            }
        },
    )?;
    command("meow-undo", "Undo.", |_| {
        cancel(&cur());
        run_command("undo")
    })?;
    command("meow-undo-in-selection", "Undo while preserving the current selection.", |_| {
        run_command("undo")
    })?;
    command("meow-change-char", "Delete the character at point and enter insert state.", |_| {
        let b = cur();
        if b.point().column < char_len(&b.line(b.point().line)) {
            run_command("delete-char")?;
            insert_for(&b, "change");
        }
        Ok(())
    })?;
    command(
        "meow-yank-pop",
        "Replace the last Meow yank with the next saved item.\n\nWith meow-use-clipboard on this steps through the editor's own kill ring, the way M-y does.",
        |_| {
            let b = cur();
            // Only straight after a yank (or replace) of ours, with point at the
            // end of what it put in: that text is what gets replaced.
            let last = LAST_YANK
                .with(Cell::get)
                .filter(|(id, _, end)| *id == b.id() && same(*end, b.point()));
            let Some((_, start, end)) = last else {
                message("Previous command was not a Meow yank");
                return Ok(());
            };
            let text = if config::with(|o| o.use_clipboard) {
                // The editor's ring: M-y steps to the next-older entry.
                kill_ring::rotate()
            } else {
                let items = KILL_RING.with(|ring| ring.borrow().clone());
                if items.is_empty() {
                    return run_command("yank-pop");
                }
                let index = KILL_RING_INDEX.with(|i| (i.get() + 1) % items.len());
                KILL_RING_INDEX.with(|i| i.set(index));
                Some(items[index].clone())
            };
            let Some(text) = text else {
                message("The kill ring is empty");
                return Ok(());
            };
            b.delete(Span { start, end });
            insert_yank(&b, &text);
            Ok(())
        },
    )?;
    for (name, keys, doc) in [
        ("meow-C-k", "C-k", "Run the editor's C-k command."),
        ("meow-C-d", "C-d", "Run the editor's C-d command."),
        ("meow-delete", "C-d", "Delete forward, or delete the selection."),
        ("meow-backward-delete", "DEL", "Delete backward, or delete the selection."),
        ("meow-backspace", "DEL", "Delete backward, or delete the selection."),
        ("meow-kill-word", "M-d", "Kill the word after point."),
        ("meow-backward-kill-word", "M-DEL", "Kill the word before point."),
        ("meow-indent", "TAB", "Indent the current line or selection."),
        ("meow-back-to-indentation", "M-m", "Move to the first non-whitespace character."),
        ("meow-M-x", "M-x", "Run an extended command."),
        ("meow-comment", "M-;", "Comment or uncomment the current line or selection."),
    ] {
        command(name, doc, move |_| execute_keys(keys))?;
    }
    command("meow-kill-symbol", "Kill the symbol after point.", |n| {
        kill_chars(true, false, n)
    })?;
    command("meow-backward-kill-symbol", "Kill the symbol before point.", |n| {
        kill_chars(true, true, n)
    })?;
    command("meow-kill-whole-line", "Kill the current line, newline and all.", |n| {
        for _ in 0..n.unwrap_or(1).max(1) {
            kill_whole_line()?;
        }
        Ok(())
    })?;
    command(
        "meow-kill-append",
        "Kill the selection and append it to the kill ring.\n\nWith no selection this says \"No selection\", as Meow does.",
        |n| {
            with_selection_fallback("meow-kill-append", || {
                for _ in 0..n.unwrap_or(1).max(1) {
                    kill_append()?;
                }
                Ok(())
            })
        },
    )?;
    command("meow-save-append", "Append the selection to the saved text.", |_| {
        let b = cur();
        let Some(region) = b.region() else {
            message("Nothing selected");
            return Ok(());
        };
        let text = b.text_in(region);
        // Only Meow's ring is touched: the editor's already holds the region
        // when the selection was made, and recording it again would show the
        // text twice in a yank-pop cycle across the two rings.
        KILL_RING.with(|ring| {
            let mut ring = ring.borrow_mut();
            if let Some(last) = ring.first_mut() {
                last.push_str(&text);
            } else {
                ring.push(text);
            }
        });
        KILL_RING_INDEX.with(|i| i.set(0));
        Ok(())
    })?;
    command("meow-save-empty", "Save an empty string in the Meow kill ring.", |_| {
        KILL_RING.with(|ring| ring.borrow_mut().insert(0, String::new()));
        KILL_RING_INDEX.with(|i| i.set(0));
        Ok(())
    })?;
    command("meow-save-char", "Save the character at point.", |_| {
        let b = cur();
        let p = b.point();
        let c = b.line(p.line).chars().nth(p.column as usize);
        if let Some(c) = c {
            KILL_RING.with(|ring| ring.borrow_mut().insert(0, c.to_string()));
            KILL_RING_INDEX.with(|i| i.set(0));
        }
        Ok(())
    })?;
    command(
        "meow-replace",
        "Replace the selection with the last saved text.\n\nWith no selection this says \"No selection\", as Meow does.",
        |_| {
            with_selection_fallback("meow-replace", || {
                let b = cur();
                let Some(text) = yank_text() else {
                    message("Meow kill ring is empty");
                    return Ok(());
                };
                if let Some(region) = b.region() {
                    b.delete(region);
                }
                insert_yank(&b, text.trim_end_matches('\n'));
                REPLACE_START.with(|r| r.set(LAST_YANK.with(Cell::get)));
                Ok(())
            })
        },
    )?;
    command("meow-replace-char", "Replace the character at point with saved text.", |_| {
        let b = cur();
        let Some(text) = yank_text() else {
            message("Meow kill ring is empty");
            return Ok(());
        };
        let p = b.point();
        if p.column < char_len(&b.line(p.line)) {
            let end = pos(p.line, p.column + 1);
            b.replace(Span { start: p, end }, text.trim_end_matches('\n'));
            REPLACE_START.with(|r| r.set(Some((b.id(), p, b.point()))));
        }
        Ok(())
    })?;
    command("meow-replace-save", "Replace the selection and save replaced text.", |_| {
        let b = cur();
        let Some(region) = b.region() else {
            return Err("Nothing selected".into());
        };
        let old = b.text_in(region);
        let Some(new) = yank_text() else {
            message("Meow kill ring is empty");
            return Ok(());
        };
        b.delete(region);
        insert_yank(&b, new.trim_end_matches('\n'));
        REPLACE_START.with(|r| r.set(LAST_YANK.with(Cell::get)));
        KILL_RING.with(|ring| ring.borrow_mut().insert(0, old));
        Ok(())
    })?;
    command(
        "meow-replace-pop",
        "Replace the last replacement with the next item in the kill ring.\n\nThis only works straight after meow-replace, meow-replace-char, meow-replace-save or itself; which one it was decides where in the ring it starts (meow-replace-pop-command-start-indexes).",
        |_| {
            let items = kill_ring_items();
            if items.is_empty() {
                return Err("Can't replace; kill ring is empty".into());
            }
            let last = previous_command();
            let index = if last.as_deref() == Some("meow-replace-pop") {
                REPLACE_POP_INDEX.with(|i| i.get() + 1)
            } else {
                let start = last
                    .as_deref()
                    .and_then(|name| config::with(|o| o.replace_pop_index(name)))
                    .map(|i| i as usize);
                match start {
                    Some(start) => start,
                    None => {
                        return Err(
                            "Can only run `meow-replace-pop' after itself or a command in `meow-replace-pop-command-start-indexes'"
                                .into(),
                        );
                    }
                }
            };
            // Past the end of the ring is the newest entry again.
            let index = if index >= items.len() { 0 } else { index };
            REPLACE_POP_INDEX.with(|i| i.set(index));
            let text = items[index].clone();
            let b = cur();
            let Some((id, start, end)) = REPLACE_START.with(Cell::get) else {
                return Err("No Meow replacement to pop".into());
            };
            if id != b.id() {
                return Err("The Meow replacement was in another buffer".into());
            }
            b.delete(Span { start, end });
            insert_yank(&b, &text);
            REPLACE_START.with(|r| r.set(LAST_YANK.with(Cell::get)));
            Ok(())
        },
    )?;
    // ------------------------------------------- structural editing (sexps)
    // Meow runs Emacs's own C-) C-( C-} C-{ M-r M-t M-s M-J and friends.
    // Forge binds none of those, so each op is worked out here instead.
    for (name, doc, op) in [
        ("meow-forward-slurp", "Slurp the next form into the pair at point: (a b) c -> (a b c).", 0),
        ("meow-backward-slurp", "Slurp the previous form into the pair at point: c (a b) -> (c a b).", 1),
        ("meow-forward-barf", "Push the last form out of the pair at point: (a b c) d -> (a b) c d.", 2),
        ("meow-backward-barf", "Push the first form out of the pair at point: (a b c) d -> a (b c) d.", 3),
        ("meow-raise-sexp", "Lift the pair at point into the place of the one around it.", 4),
        ("meow-transpose-sexp", "Swap the two forms either side of point.", 5),
        ("meow-split-sexp", "Split the pair or word at point with a space.", 6),
        ("meow-join-sexp", "Join the two forms either side of point: (a | b) -> (a b).", 7),
        ("meow-splice-sexp", "Remove the brackets of the pair at point, keeping what is inside.", 8),
    ] {
        command(name, doc, move |_| {
            let b = cur();
            cancel(&b);
            let text = flat_text(&b);
            let at = offset_of(&b, b.point());
            let plan = match op {
                0 => plan_slurp(&text, at, true),
                1 => plan_slurp(&text, at, false),
                2 => plan_barf(&text, at, true),
                3 => plan_barf(&text, at, false),
                4 => plan_raise(&text, at),
                5 => plan_transpose(&text, at),
                6 => Some(plan_split(&text, at)),
                7 => plan_join(&text, at),
                _ => plan_splice(&text, at),
            };
            run_plan(&b, plan, "No pair to work on here");
            Ok(())
        })?;
    }
    for (name, open, close, doc) in [
        ("meow-wrap-round", '(', ')', "Wrap the selection (or the form at point) in round brackets."),
        ("meow-wrap-square", '[', ']', "Wrap the selection (or the form at point) in square brackets."),
        ("meow-wrap-curly", '{', '}', "Wrap the selection (or the form at point) in curly brackets."),
        ("meow-wrap-string", '"', '"', "Wrap the selection (or the form at point) in quotes."),
    ] {
        command(name, doc, move |_| {
            let b = cur();
            let span = b.region().map_or_else(
                || {
                    let text = flat_text(&b);
                    let at = offset_of(&b, b.point());
                    form_at(&text, at).unwrap_or((at, at))
                },
                |r| (offset_of(&b, r.start), offset_of(&b, r.end)),
            );
            apply(&b, &plan_wrap(&flat_text(&b), span, open, close));
            Ok(())
        })?;
    }

    // ------------------------------------------------ thing-based commands
    for (name, doc, mode) in [
        ("meow-mark-thing", "Select the thing at point (reads the thing's key).", 0),
        ("meow-next-thing", "Select to the next thing of the same kind (reads the thing's key).", 1),
        ("meow-kill-thing", "Kill the thing at point (reads the thing's key).", 2),
    ] {
        command(name, doc, move |n| {
            let prompt = match mode {
                0 => "Mark thing",
                1 => "Next thing",
                _ => "Kill thing",
            };
            read_key(&thing_prompt_text(prompt), move |key| {
                let Some(key) = key else { return };
                let Some(thing) = key.chars().next() else { return };
                let Some(name) = thing_name(thing) else {
                    return message(&format!("No such thing: {thing}"));
                };
                let b = cur();
                let text = flat_text(&b);
                let at = offset_of(&b, b.point());
                let span = match mode {
                    1 => next_thing(&text, at, &name),
                    _ => thing_span(&b, &name, false),
                };
                let Some((start, end)) = span else {
                    return message("No such thing here");
                };
                let backward = n.unwrap_or(1) < 0;
                // Meow's `meow-next-thing-include-syntax` says which syntax
                // classes the selection takes in.
                let (start, end) = match mode {
                    1 => match include_syntax(&name, !backward) {
                        Some(syntax) => {
                            thing_span_with_syntax(&text, start, end, backward, &syntax)
                        }
                        None => (start, end),
                    },
                    _ => (start, end),
                };
                let start = position_at(&b, start);
                let end = position_at(&b, end);
                if mode == 2 {
                    // Select the thing, then kill the selection: the editor's
                    // own ring gets it too.
                    select_span(&b, start, end);
                    record_kill(b.text_in(Span { start, end }));
                    if let Err(e) = run_command("kill-region") {
                        message(&e);
                    }
                } else {
                    select_oriented(&b, start, end, backward);
                }
            });
            Ok(())
        })?;
    }

    command(
        "meow-insert",
        "Insert before the selection (or at point).",
        |_| {
            let b = cur();
            if let Some(r) = b.region() {
                b.set_point(r.start);
            }
            cancel(&b);
            insert_for(&b, "insert");
            Ok(())
        },
    )?;
    command(
        "meow-append",
        "Insert after the selection (or after the character at point).",
        |_| {
            let b = cur();
            match b.region() {
                Some(r) => b.set_point(r.end),
                None => {
                    let p = b.point();
                    if p.column < char_len(&b.line(p.line)) {
                        b.set_point(pos(p.line, p.column + 1));
                    }
                }
            }
            cancel(&b);
            insert_for(&b, "append");
            Ok(())
        },
    )?;
    command(
        "meow-open-below",
        "Open a line below and insert there.",
        |_| {
            let b = cur();
            cancel(&b);
            let line = b.point().line;
            b.set_point(pos(line, char_len(&b.line(line))));
            b.insert("\n");
            b.indent_line(line + 1);
            insert_for(&b, "open");
            Ok(())
        },
    )?;
    command(
        "meow-open-above",
        "Open a line above and insert there.",
        |_| {
            let b = cur();
            cancel(&b);
            let line = b.point().line;
            b.set_point(pos(line, 0));
            b.insert("\n");
            b.set_point(pos(line, 0));
            b.indent_line(line);
            insert_for(&b, "open");
            Ok(())
        },
    )?;
    command("meow-line-expand", "Expand the line selection.", |n| {
        run_n("meow-line", n)
    })?;
    command("meow-visual-line", "Select a visual line.", |n| {
        run_n("meow-line", n)
    })?;
    command("meow-visual-line-expand", "Expand the visual-line selection.", |n| {
        run_n("meow-line", n)
    })?;
    command("meow-goto-line", "Go to a line and select it.", |_| {
        // `meow-goto-line-function` says which command to use; by default
        // the editor's own.
        let command = config::with(|o| o.goto_line_function.clone())
            .unwrap_or_else(|| "goto-line".to_string());
        run_command(&command)
    })?;
    command("meow-block", "Select the enclosing or next balanced block.", |_| {
        let b = cur();
        let p = b.point();
        if let Some((start, end)) = thing_bounds(&b, "round", false)
            .or_else(|| thing_bounds(&b, "square", false))
            .or_else(|| thing_bounds(&b, "curly", false))
        {
            select_span(&b, start, end);
            Ok(())
        } else {
            message("No surrounding block");
            let _ = p;
            Ok(())
        }
    })?;
    command("meow-to-block", "Select from point to the next balanced block.", |_| {
        let b = cur();
        let start = b.point();
        let text = flat_text(&b);
        if let Some((_, close)) = pair_from(&text, offset_of(&b, start)) {
            select_span(&b, start, position_at(&b, close));
        } else {
            message("No following block");
        }
        Ok(())
    })?;
    command("meow-join", "Select whitespace between adjacent non-empty lines.", |n| {
        let b = cur();
        let line = b.point().line as i64;
        let direction = n.unwrap_or(1);
        let (from_line, to_line) = if direction < 0 {
            (line, line + 1)
        } else {
            (line - 1, line)
        };
        if from_line < 0 || to_line >= b.line_count() as i64 {
            message("No adjacent line to join");
            return Ok(());
        }
        let mut previous = from_line;
        while previous >= 0 && b.line(previous as u32).trim().is_empty() {
            previous -= 1;
        }
        let mut next = to_line;
        while next < b.line_count() as i64 && b.line(next as u32).trim().is_empty() {
            next += 1;
        }
        if previous < 0 || next >= b.line_count() as i64 {
            message("No adjacent non-empty line");
            return Ok(());
        }
        let left = b.line(previous as u32);
        let right = b.line(next as u32);
        let indent = right.chars().take_while(|c| c.is_whitespace()).count();
        select_span(
            &b,
            pos(previous as u32, char_len(&left)),
            pos(next as u32, indent as u32),
        );
        Ok(())
    })?;
    for (name, operation, doc) in [
        ("meow-inner-of-thing", 0, "Select inside a thing."),
        ("meow-bounds-of-thing", 1, "Select a thing including delimiters."),
        ("meow-beginning-of-thing", 2, "Select from point to the beginning of a thing."),
        ("meow-end-of-thing", 3, "Select from point to the end of a thing."),
    ] {
        command(name, doc, move |_| {
            let which = match operation {
                0 => "inner",
                1 => "bounds",
                2 => "beginning",
                _ => "end",
            };
            // The direction each of these selects in is the user's to choose.
            let backward = config::with(|o| o.thing_direction(which) == "backward");
            read_key(&thing_prompt_text("Thing"), move |key| {
                let Some(key) = key else { return };
                let Some(thing) = key.chars().next() else { return };
                let Some(thing) = thing_name(thing) else {
                    message(&format!("No such thing: {}", key.chars().next().unwrap_or(' ')));
                    return;
                };
                let b = cur();
                let point = b.point();
                let bounds = thing_bounds(&b, &thing, operation == 0);
                let Some((start, end)) = bounds else {
                    message("No such thing at point");
                    return;
                };
                match operation {
                    0 | 1 => select_oriented(&b, start, end, backward),
                    2 if offset_of(&b, point) <= offset_of(&b, start) => {
                        select_oriented(&b, point, start, backward)
                    }
                    2 => select_oriented(&b, point, end, backward),
                    3 if offset_of(&b, point) <= offset_of(&b, end) => {
                        select_oriented(&b, point, end, backward)
                    }
                    _ => select_oriented(&b, point, start, backward),
                }
            });
            Ok(())
        })?;
    }
    command(
        "meow-expand",
        "Expand the current selection by what it is made of: a word selection grows by words, a line selection by lines.\n\nWith no selection this runs the fallback for meow-expand, which is meow-digit-argument unless you set another.",
        |n| {
            with_selection_fallback("meow-expand", || {
                expand(n.unwrap_or(1));
                Ok(())
            })
        },
    )?;
    for n in 0..=9 {
        let name = format!("meow-expand-{n}");
        let doc = format!(
            "Expand the current selection by {n}.\n\nWith no selection this says \"No selection\", as Meow does."
        );
        let fallback = name.clone();
        command(&name, &doc, move |_| {
            with_selection_fallback(&fallback, || {
                // Meow's meow-expand-0 expands by ten.
                expand(if n == 0 { 10 } else { n as i64 });
                Ok(())
            })
        })?;
    }
    command(
        "meow-visit",
        "Search for a regular expression and select the match.\n\nThe words and symbols of the buffer are offered for completion. With meow-visit-sanitize-completion on (the default) what you pick is searched for literally; with it off, what you type is used as a regular expression.",
        |n| {
            let b = cur();
            let id = b.id();
            let backward = n.unwrap_or(1) < 0;
            let prompt = if backward { "Visit backward: " } else { "Visit: " };
            let candidates = visit_candidates(&b);
            completing_read(prompt, &candidates, false, move |answer| {
                let Some(answer) = answer else { return };
                let answer = answer.trim().to_string();
                if answer.is_empty() {
                    return;
                }
                // Meow's `meow-visit-sanitize-completion`: on, the text is
                // searched for literally; off, it is a regular expression.
                let pattern = if config::with(|o| o.visit_sanitize_completion) {
                    escape_regexp(&answer)
                } else {
                    answer.clone()
                };
                SEARCH_RING.with(|ring| ring.borrow_mut().insert(0, answer.clone()));
                let Some(b) = Buffer::from_id(id) else { return };
                let found = if backward {
                    search_backward(&b, &pattern)
                } else {
                    b.search_regexp(&pattern, b.point())
                };
                match found {
                    Ok(Some(span)) => select_span(&b, span.start, span.end),
                    Ok(None) => message("No match"),
                    Err(e) => message(&format!("Invalid regexp: {e}")),
                }
            });
            Ok(())
        },
    )?;
    command("meow-search", "Search again for the most recent Meow regexp.", |n| {
        let Some(pattern) = SEARCH_RING.with(|ring| ring.borrow().first().cloned()) else {
            message("No previous Meow search");
            return Ok(());
        };
        let b = cur();
        let (low, high) = selection_bounds(&b);
        let result = if n.unwrap_or(1) < 0 {
            // The match before the selection: the last one that starts
            // before it (the buffer can only search forward).
            match b.search_regexp(&pattern, pos(0, 0)) {
                Err(e) => Err(e),
                Ok(_) => Ok(previous_match(
                    |from| b.search_regexp(&pattern, from).ok().flatten(),
                    |p| {
                        if p.column < char_len(&b.line(p.line)) {
                            pos(p.line, p.column + 1)
                        } else if p.line + 1 < b.line_count() {
                            pos(p.line + 1, 0)
                        } else {
                            p
                        }
                    },
                    low,
                )),
            }
        } else {
            // From the end of the selection, so a backward one doesn't find
            // itself again.
            b.search_regexp(&pattern, high)
        };
        match result {
            Ok(Some(span)) => select_span(&b, span.start, span.end),
            Ok(None) => message("No match"),
            Err(e) => message(&format!("Invalid regexp: {e}")),
        }
        Ok(())
    })?;
    command("meow-pop-search", "Remove the latest regexp from the search ring.", |_| {
        let popped = SEARCH_RING.with(|ring| {
            let mut ring = ring.borrow_mut();
            (!ring.is_empty()).then(|| ring.remove(0))
        });
        match popped {
            Some(pattern) => message(&format!("Popped search: {pattern}")),
            None => message("No previous Meow search"),
        }
        cancel(&cur());
        Ok(())
    })?;
    command(
        "meow-pop-selection",
        "Restore the previous Meow selection.\n\nWith no selection this runs the fallback for meow-pop-selection, which is meow-pop-grab unless you set another.",
        |_| {
            with_selection_fallback("meow-pop-selection", || {
                let previous = SELECTION_RING.with(|ring| ring.borrow_mut().pop());
                if let Some((id, mark, point)) = previous {
                    if let Some(b) = Buffer::from_id(id) {
                        b.show();
                        b.set_mark(Some(mark));
                        b.set_point(point);
                    }
                }
                Ok(())
            })
        },
    )?;
    command("meow-pop-all-selection", "Clear the selection history.", |_| {
        SELECTION_RING.with(|ring| ring.borrow_mut().clear());
        cancel(&cur());
        Ok(())
    })?;
    command("meow-grab", "Save the current selection as a secondary grab.", |_| {
        let b = cur();
        let point = b.point();
        let region = b.region();
        let (start, end, text) = match region {
            Some(span) => (span.start, span.end, b.text_in(span)),
            None => (point, point, String::new()),
        };
        GRAB.with(|grab| *grab.borrow_mut() = Some((b.id(), start, end, text)));
        cancel(&b);
        Ok(())
    })?;
    command(
        "meow-sync-grab",
        "Replace the saved grab with the current selection.\n\nWith no selection this says \"No selection\", as Meow does.",
        |_| {
            with_selection_fallback("meow-sync-grab", || {
                let b = cur();
                let Some(span) = b.region() else {
                    return Ok(());
                };
                let text = b.text_in(span);
                GRAB.with(|grab| {
                    *grab.borrow_mut() = Some((b.id(), span.start, span.end, text));
                });
                cancel(&b);
                Ok(())
            })
        },
    )?;
    command("meow-swap-grab", "Swap the current selection with the saved grab.", |_| {
        let b = cur();
        let Some(span) = b.region() else {
            message("Nothing selected");
            return Ok(());
        };
        let Some((id, grab_start, grab_end, _)) = GRAB.with(|grab| grab.borrow().clone()) else {
            message("No saved grab");
            return Ok(());
        };
        let old = b.text_in(span);
        let grabbed_span = Span { start: grab_start, end: grab_end };
        let new_grab = if id == b.id() {
            // The same buffer: both stretches change places in one edit.
            let text = flat_text(&b);
            let sel = (offset_of(&b, span.start), offset_of(&b, span.end));
            let grab = (offset_of(&b, grab_start), offset_of(&b, grab_end));
            let Some((plan, sel_now, _)) = plan_swap(&text, sel, grab) else {
                message("The selection and the grab overlap");
                return Ok(());
            };
            apply(&b, &plan);
            (b.id(), position_at(&b, sel_now.0), position_at(&b, sel_now.1))
        } else {
            let Some(other) = Buffer::from_id(id) else {
                message("The buffer for the saved grab was closed");
                return Ok(());
            };
            let grabbed = other.text_in(grabbed_span);
            b.replace(span, &grabbed);
            other.replace(grabbed_span, &old);
            let end = offset_of(&other, grab_start) + old.chars().count();
            (id, grab_start, position_at(&other, end))
        };
        GRAB.with(|grab| *grab.borrow_mut() = Some((new_grab.0, new_grab.1, new_grab.2, old)));
        cancel(&b);
        Ok(())
    })?;
    command("meow-pop-grab", "Return to the saved grab location.", |_| {
        let Some((id, start, end, _)) = GRAB.with(|grab| grab.borrow().clone()) else {
            message("No saved grab");
            return Ok(());
        };
        if let Some(b) = Buffer::from_id(id) {
            b.show();
            b.set_mark(Some(start));
            b.set_point(end);
        } else {
            message("The buffer for the saved grab was closed");
        }
        Ok(())
    })?;
    command("meow-pop-to-mark", "Jump to the previous mark in this buffer.", |_| {
        jump_to_mark(true)
    })?;
    command("meow-unpop-to-mark", "Reverse the previous mark jump.", |_| {
        unjump_to_mark()
    })?;
    command(
        "meow-pop-or-unpop-to-mark",
        "Jump to the previous mark, or back from it.\n\nA negative argument goes back. With meow-pop-or-unpop-to-mark-repeat-unpop on, going back twice in a row goes back again.",
        |n| {
            let negative = n.unwrap_or(1) < 0;
            let repeat_unpop = config::with(|o| o.pop_or_unpop_to_mark_repeat_unpop);
            let last = previous_command();
            if negative || (repeat_unpop && last.as_deref() == Some("meow-unpop-to-mark")) {
                // Meow counts this as the unpop command, so that a repeat
                // can unpop again.
                LAST_COMMAND.with(|c| *c.borrow_mut() = Some("meow-unpop-to-mark".into()));
                unjump_to_mark()
            } else {
                jump_to_mark(true)
            }
        },
    )?;
    command("meow-pop-to-global-mark", "Jump to the previous mark, in any buffer.", |_| {
        jump_to_mark(false)
    })?;
    command("meow-insert-exit", "Back to normal state.", |_| {
        let b = cur();
        normal(&b);
        select_inserted(&b);
        Ok(())
    })?;
    command(
        "meow-undefined",
        "Keys normal state doesn't use (they don't insert).",
        |_| {
            message("Not bound in normal state (i to insert, SPC for the keypad)");
            Ok(())
        },
    )?;
    // ---------------------------------------------------- global and misc
    // Meow names these for the keypad and the layers; most are a thin
    // wrapper over the editor's own command of the same name.
    for (name, builtin, doc) in [
        ("meow-quit", "quit-window", "Close the window, or ask whether to quit."),
        ("meow-keyboard-quit", "keyboard-quit", "Abandon what is in progress: a prompt, or a selection."),
        ("meow-minibuffer-quit", "keyboard-quit", "Abandon the minibuffer."),
        ("meow-query-replace", "query-replace", "Search for a string and replace it."),
        ("meow-query-replace-regexp", "query-replace-regexp", "Search for a regular expression and replace it."),
        ("meow-last-buffer", "switch-to-buffer", "Visit another buffer (Forge has no previous-buffer, so this is the buffer list)."),
        ("meow-eval-last-exp", "eval-expression", "Read an expression and evaluate it."),
        ("meow-end-kmacro", "end-kbd-macro", "Stop recording a keyboard macro."),
        ("meow-end-or-call-kmacro", "kmacro-end-or-call-macro", "Stop recording, or run the macro."),
        ("meow-kmacro-lines", "start-kbd-macro", "Record a keyboard macro while the line moves."),
        ("meow-kmacro-matches", "call-last-kbd-macro", "Run the last keyboard macro again."),
    ] {
        command(name, doc, move |n| {
            // Meow's kmacro-lines records while the line moves, so any macro
            // in progress is finished first.
            if builtin == "start-kbd-macro" {
                run_command("kmacro-end-or-call-macro")?;
            }
            run_n(builtin, n)
        })?;
    }
    command("meow-repeat-last-key", "Run the last normal-state key again.", |_| {
        // Forge has no `repeat` command, so this asks for the key itself:
        // the keypad is the honest way to say "that one, again".
        read_key("Repeat: ", |key| {
            if let Some(key) = key {
                let _ = execute_keys(&key);
            }
        });
        Ok(())
    })?;
    command("meow-leader", "Type a count for the next key (digits, then the key).", |_| {
        digit_argument();
        Ok(())
    })?;
    for key in ["M-1", "M-2", "M-3", "M-4", "M-5", "M-6", "M-7", "M-8", "M-9"] {
        bind_in_mode(NORMAL, key, "meow-digit-argument")?;
    }
    command("meow-motion-state", "Use this mode's own keys, with SPC for the keypad.", |_| {
        motion(&cur());
        Ok(())
    })?;
    command("meow-universal-argument", "Start a numeric argument: C-u is 4, C-u 12 is 12.", |_| {
        run_command("universal-argument")
    })?;
    command("meow-digit-argument", "Read digits, then the key to run them on (3 d deletes three).", |_| {
        digit_argument();
        Ok(())
    })?;
    for name in ["meow-describe-key", "meow-keypad-describe-key"] {
        command(name, "Read one key and say what it is bound to.", |_| {
            describe_key();
            Ok(())
        })?;
    }
    command("meow-cheatsheet", "List what the normal-state keys run.", |_| {
        // There is no way to read a keymap back out of the editor, so this
        // reads the layer the plugin itself put in, which is what it bound.
        let name = LAYOUT.with(Cell::get);
        let Some(layout) = layout::layout(name) else {
            message("No Meow layout");
            return Ok(());
        };
        let keys: Vec<String> = layout::normal_keys(layout)
            .iter()
            .map(|(key, command)| format!("{key:<3} {command}"))
            .collect();
        message(&keys.join("  "));
        Ok(())
    })?;
    for layout in layout::LAYOUTS {
        let name = layout.name;
        let doc = format!(
            "Use the {name} layout: the same commands as Meow, on the keys of that keyboard.\n\nThe layouts are {}.",
            layout::names().join(", ")
        );
        command(&format!("meow-setup-{name}"), &doc, move |_| {
            setup_layout(name)
        })?;
    }
    command("meow-setup", "Use the QWERTY layout, the one Meow starts with.", |_| {
        setup_layout("qwerty")
    })?;
    command("meow-thing-prompt", "Ask which thing a thing-based command should use.", |_| {
        read_key(&thing_prompt_text("Thing"), |key| {
            if let Some(c) = key.and_then(|k| k.chars().next())
                && thing_name(c).is_none()
            {
                message(&format!("No such thing: {c}"));
            }
        });
        Ok(())
    })?;
    command("meow-temp-normal", "Back to normal state for one key, then to the state you were in.", |_| {
        let b = cur();
        let was_insert = b.minor_modes().contains(&INSERT.to_string());
        normal(&b);
        read_key("Normal for one key: ", move |key| {
            // The key is run in normal state, then insert state comes back
            // unless that key moved to another state itself.
            if let Some(key) = key {
                let _ = execute_keys(&key);
            }
            if was_insert && b.alive() && b.minor_modes().contains(&NORMAL.to_string()) {
                insert(&b);
            }
        });
        Ok(())
    })?;
    command("meow-escape-or-normal-modal", "Leave insert state, or cancel the selection.", |_| {
        let b = cur();
        if b.minor_modes().contains(&INSERT.to_string()) {
            cancel(&b);
            normal(&b);
            select_inserted(&b);
        } else {
            cancel(&b);
        }
        Ok(())
    })?;
    command("meow-pop-marker", "Go back to where point was before the last motion.", |_| {
        jump_to_mark(true)
    })?;
    command("meow-find-ref", "Search again for the character of the last find or till.", |n| {
        let Some(c) = LAST_CHAR.with(|cell| cell.get()) else {
            message("No previous find");
            return Ok(());
        };
        let b = cur();
        let text = flat_text(&b);
        let at = offset_of(&b, b.point());
        let backward = n.unwrap_or(1) < 0;
        let Some(target) = find_ref(&text, at, c, backward) else {
            message(&format!("No {c} {} point", if backward { "before" } else { "after" }));
            return Ok(());
        };
        select_from_point(&b);
        b.set_point(position_at(&b, target));
        Ok(())
    })?;
    command("meow-change-save", "Change the selection and put what was there into the kill ring.", |_| {
        let b = cur();
        if let Some(region) = b.region() {
            let killed = b.text_in(region);
            KILL_RING.with(|ring| ring.borrow_mut().insert(0, killed));
            run_command("kill-region")?;
        } else {
            run_command("delete-char")?;
        }
        insert(&b);
        Ok(())
    })?;
    command("meow-open-below-visual", "Open a line below the selection and insert there.", |_| {
        open_line(true, true)
    })?;
    command("meow-open-above-visual", "Open a line above the selection and insert there.", |_| {
        open_line(false, true)
    })?;
command(
        "meow-keypad",
        "Type an Emacs key sequence without chords: SPC x f is C-x C-f.",
        |_| open_keypad(),
    )?;
    // One command per key the keypad offers, so the transient map can bind
    // a real key sequence to each. They all read the same state.
    for (n, key) in keypad_keys().into_iter().enumerate() {
        command(
            &keypad_command(n),
            &format!("Keypad key {key}."),
            move |_| keypad_press(&key),
        )?;
    }
    command(
        "meow-keypad-quit",
        "Close the keypad without running anything.",
        |_| {
            quit_keypad();
            Ok(())
        },
    )?;
    for name in ["meow-keypad-start", "meow-keypad-start-with"] {
        command(name, "Open the keypad again, wherever it was left.", |_| {
            quit_keypad();
            open_keypad()
        })?;
    }
    command(
        "meow-mode",
        "Turn modal editing on or off everywhere.",
        |_| {
            let on = !ENABLED.with(Cell::get);
            ENABLED.with(|e| e.set(on));
            for b in Buffer::all() {
                if on {
                    setup(&b);
                } else {
                    off(&b);
                }
            }
            message(&format!(
                "Meow mode {}",
                if on { "enabled" } else { "disabled" }
            ));
            Ok(())
        },
    )?;
    command(
        "meow-normal-state",
        "Switch this buffer to normal state.",
        |_| {
            normal(&cur());
            Ok(())
        },
    )?;
    command(
        "meow-insert-state",
        "Switch this buffer to insert state.",
        |_| {
            insert(&cur());
            Ok(())
        },
    )?;

    // Meow's QWERTY layout (KEYBINDING_QWERTY.org in Meow itself), with the
    // two bindings this editor's readers expect kept where Meow leaves the
    // keys free.
    extra::init()?;
    webkit::init()?;
    setup_layout("qwerty")?;
    // The leader layer is where a count comes from: digits, then a key.
    for c in '0'..='9' {
        bind_in_mode(LEADER, &c.to_string(), "meow-digit-argument")?;
    }
    bind_in_mode(LEADER, "/", "meow-keypad-describe-key")?;
    bind_in_mode(LEADER, "?", "meow-cheatsheet")?;
    // Motion state is for modes with keys of their own: it leaves them
    // alone and only puts the keypad on SPC. Add motion keys with
    // `meow-motion-define-key` if a mode wants more.
    bind_motion("SPC", "meow-keypad")?;
    bind_in_mode(INSERT, "ESC", "meow-insert-exit")?;


    on(EventKind::BufferOpened, |e| {
        if let Some(b) = Buffer::from_id(forge_plugin_sdk::event_buffer(e)) {
            setup(&b);
        }
    });
    on(EventKind::ModeChanged, |e| {
        if let Some(b) = Buffer::from_id(forge_plugin_sdk::event_buffer(e)) {
            setup(&b);
        }
    });
    for b in Buffer::all() {
        setup(&b);
    }
    Ok(())
}

forge_plugin!(init);

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn mode_states_follow_the_mode_list_and_derived_modes() {
        let parent = |mode: &str| match mode {
            "webkit-mode" | "help-mode" => Some("special-mode".to_string()),
            "grep-mode" => Some("compilation-mode".to_string()),
            "compilation-mode" => Some("special-mode".to_string()),
            _ => None,
        };
        assert_eq!(state_for_mode("webkit-mode", parent), "motion");
        assert_eq!(state_for_mode("help-mode", parent), "motion");
        assert_eq!(state_for_mode("grep-mode", parent), "motion");
        assert_eq!(state_for_mode("text-mode", parent), "normal");
        assert_eq!(state_for_mode("fundamental-mode", parent), "normal");
        assert_eq!(state_for_mode("rust-mode", parent), "normal");
    }

    #[test]
    fn words_and_finding() {
        assert_eq!(word_bounds("hello world", 1), (0, 5));
        assert_eq!(
            word_bounds("hello world", 5),
            (6, 11),
            "off a word: the next one"
        );
        assert_eq!(word_bounds("a  ", 2), (3, 3));
        assert_eq!(word_bounds("héllo", 4), (0, 5));
        assert_eq!(symbol_bounds("foo_bar-baz", 4), (0, 11));
        assert_eq!(symbol_bounds("foo_bar-baz", 8), (0, 11));
        assert_eq!(find_char("a,b,c", 2, ',', true), Some(4));
        assert_eq!(find_char("a,b,c", 2, ',', false), Some(3));
        assert_eq!(find_char("abc", 0, 'z', true), None);
    }

    #[test]
    fn keypad_translates_like_meow() {
        assert_eq!(
            keypad_step(vec![], false, None, "x"),
            Keypad::Try {
                seq: v(&["C-x"]),
                plain: None
            }
        );
        assert_eq!(
            keypad_step(v(&["C-x"]), false, None, "f"),
            Keypad::Try {
                seq: v(&["C-x", "C-f"]),
                plain: Some(v(&["C-x", "f"]))
            }
        );
        assert_eq!(
            keypad_step(vec![], false, None, "m"),
            Keypad::More {
                seq: vec![],
                literal: false,
                modifier: Some("M-")
            }
        );
        assert_eq!(
            keypad_step(vec![], false, Some("M-"), "f"),
            Keypad::Try {
                seq: v(&["M-f"]),
                plain: None
            }
        );
        assert_eq!(
            keypad_step(vec![], false, None, "a"),
            Keypad::Try {
                seq: v(&["C-c", "a"]),
                plain: None
            }
        );
        assert_eq!(
            keypad_step(v(&["C-x"]), false, None, "SPC"),
            Keypad::More {
                seq: v(&["C-x"]),
                literal: true,
                modifier: None
            }
        );
        assert_eq!(
            keypad_step(v(&["C-x"]), true, None, "b"),
            Keypad::Try {
                seq: v(&["C-x", "b"]),
                plain: None
            }
        );
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// Carry a plan out on plain text, the way the buffer would.
    fn run(source: &str, plan: Option<Plan>) -> Option<String> {
        let t = chars(source);
        let plan = plan?;
        let mut out: String = t[..plan.span.0.min(t.len())].iter().collect();
        out.push_str(&plan.text);
        out.extend(t.get(plan.span.1..)?.iter());
        Some(out)
    }

    #[test]
    fn pairs_are_found_and_brackets_ignore_strings() {
        let t = chars("(a (b) c)");
        assert_eq!(pairs_in(&t), [(0, 9), (3, 6)]);
        // The innermost pair containing an offset is the small one.
        assert_eq!(pair_at(&t, 4), Some((3, 6)));
        assert_eq!(pair_at(&t, 1), Some((0, 9)));
        assert_eq!(pair_at(&t, 9), None, "past the end");
        // A bracket inside a string is not a bracket.
        assert_eq!(pairs_in(&chars(r#"(a ")" b)"#)), [(0, 9)]);
        assert_eq!(pairs_in(&chars("(a")), []);
    }

    #[test]
    fn things_are_the_keys_meow_asks_for() {
        let keys: String = things().iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, "rscgewbplvd.", "Meow's meow-char-thing-table");
        assert_eq!(thing_name('r').as_deref(), Some("round"));
        assert_eq!(thing_name('.').as_deref(), Some("sentence"));
        assert_eq!(thing_name('z'), None);
    }

    #[test]
    fn structural_edits_do_what_they_say() {
        let s = chars;
        // slurp: the form moves in, and the gap with it.
        assert_eq!(run("(a b) c", plan_slurp(&s("(a b) c"), 4, true)), Some("(a b c)".into()));
        assert_eq!(run("c (a b)", plan_slurp(&s("c (a b)"), 4, false)), Some("(c a b)".into()));
        assert_eq!(run("(a b c) d", plan_barf(&s("(a b c) d"), 5, true)), Some("(a b) c d".into()));
        assert_eq!(run("(a b c) d", plan_barf(&s("(a b c) d"), 2, false)), Some("a (b c) d".into()));
        // raise lifts the inner pair into the outer's place; splice only
        // drops the brackets.
        assert_eq!(run("x(a (b c))y", plan_raise(&s("x(a (b c))y"), 6)), Some("x(b c)y".into()));
        assert_eq!(run("(a b)", plan_raise(&s("(a b)"), 2)), None, "nothing above to raise into");
        assert_eq!(run("x(a (b c))y", plan_splice(&s("x(a (b c))y"), 6)), Some("x(a b c)y".into()));
        // transpose swaps the forms either side of point.
        assert_eq!(run("(a b) (c d)", plan_transpose(&s("(a b) (c d)"), 6)), Some("(c d) (a b)".into()));
        // join brings the two forms together, however much whitespace was
        // between them.
        assert_eq!(run("(a   b)", plan_join(&s("(a   b)"), 2)), Some("(ab)".into()));
        assert_eq!(run("(a b)", plan_join(&s("(a b)"), 2)), Some("(ab)".into()));
        assert_eq!(run("(a)  (b)", plan_join(&s("(a)  (b)"), 2)), None, "between two pairs, not inside one");
        // split opens a space at point.
        assert_eq!(run("(a b)", Some(plan_split(&s("(a b)"), 2))), Some("(a  b)".into()));
        // wrap takes the span it is given.
        assert_eq!(run("a b c d", Some(plan_wrap(&s("a b c d"), (0, 5), '(', ')'))), Some("(a b c) d".into()));
        assert_eq!(run("a b c d", Some(plan_wrap(&s("a b c d"), (0, 5), '"', '"'))), Some("\"a b c\" d".into()));
        // Nothing to work on is None, so the command can say so.
        assert_eq!(plan_raise(&s("no brackets"), 0), None);
        assert_eq!(plan_slurp(&s("no brackets"), 0, true), None);
    }

    #[test]
    fn slurping_and_barfing_nested_forms() {
        let s = chars;
        // A pair with one form in it has nothing to barf out, and says so
        // rather than leaving an empty pair behind.
        assert_eq!(plan_barf(&s("(a (b) c) d"), 4, true), None);
        assert_eq!(plan_barf(&s("(a (b) c) d"), 4, false), None);
        // From the outer pair, the inner one moves as a whole.
        assert_eq!(run("(a (b c)) d", plan_barf(&s("(a (b c)) d"), 1, true)), Some("(a (b c)) d".into()));
        assert_eq!(run("(a (b c) e) f", plan_barf(&s("(a (b c) e) f"), 9, true)), Some("(a (b c)) e f".into()));
        // Slurping takes the whole next pair too.
        assert_eq!(run("(a) (b c)", plan_slurp(&s("(a) (b c)"), 1, true)), Some("(a (b c))".into()));
    }

    #[test]
    fn next_thing_steps_over_bracketed_things() {
        let t = chars("aa (bb) (cc) dd");
        assert_eq!(next_thing(&t, 0, "round"), Some((3, 7)));
        assert_eq!(next_thing(&t, 4, "round"), Some((8, 12)));
        assert_eq!(next_thing(&t, 8, "round"), Some((8, 12)));
        // Past the last pair there is nothing more.
        assert_eq!(next_thing(&t, 12, "round"), None);
        // Words step one run at a time; a bracketed run counts as one form,
        // brackets and all.
        assert_eq!(next_thing(&t, 0, "symbol"), Some((0, 2)));
        assert_eq!(next_thing(&t, 2, "symbol"), Some((3, 7)));
        assert_eq!(next_thing(&t, 13, "symbol"), Some((13, 15)));
        assert_eq!(next_thing(&t, 15, "symbol"), None);
    }

    #[test]
    fn the_qwerty_layer_is_meows() {
        let keys: Vec<&str> = layout::normal_keys(layout::layout("qwerty").unwrap())
            .iter()
            .map(|(k, _)| *k)
            .collect();
        for key in [
            "w", "W", "e", "E", "b", "B", "x", "f", "t", ";", ",", ".", "[", "]", "g", "G", "d", "D", "c", "y", "p",
            "u", "U", "i", "a", "A", "o", "O", "s", "q", "Q", "r", "R", "v", "z", "-",
        ] {
            assert!(keys.contains(&key), "{key} is in Meow's layout");
        }
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        let count = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), count, "no key bound twice");
        assert!(
            layout::normal_keys(layout::layout("qwerty").unwrap())
                .iter()
                .all(|(k, c)| !k.is_empty() && !c.is_empty())
        );
    }

    #[test]
    fn forms_before_and_around_point() {
        let c = chars;
        // The last form before point, not the first in the buffer.
        assert_eq!(form_before(&c("foo bar"), 7), Some((4, 7)));
        assert_eq!(form_before(&c("foo bar  "), 9), Some((4, 7)), "over trailing space");
        assert_eq!(form_before(&c("foo (a b) "), 10), Some((4, 9)), "a whole pair");
        assert_eq!(form_before(&c("foo ("), 5), None, "an open bracket is not a form");
        assert_eq!(form_before(&c("  "), 2), None);
        assert_eq!(form_before(&c("foo"), 0), None);
        // form_at: the word under or just before point, else the pair around.
        assert_eq!(form_at(&c("foo bar"), 7), Some((4, 7)));
        assert_eq!(form_at(&c("foo bar"), 5), Some((4, 7)));
        assert_eq!(form_at(&c("(a  b)"), 3), Some((0, 6)), "between words: the pair, brackets and all");
        assert_eq!(form_at(&c("   "), 1), None);
    }

    #[test]
    fn slurp_backward_and_transpose_use_the_neighbour() {
        let s = chars;
        // The form before the pair, wherever the pair is in the text.
        assert_eq!(run("x c (a b)", plan_slurp(&s("x c (a b)"), 6, false)), Some("x (c a b)".into()));
        assert_eq!(run("x (c) (a b)", plan_slurp(&s("x (c) (a b)"), 8, false)), Some("x ((c) a b)".into()));
        assert_eq!(run("c ()", plan_slurp(&s("c ()"), 3, false)), Some("(c)".into()));
        assert_eq!(plan_slurp(&s("(a b)"), 2, false), None, "nothing before the pair");
        // Slurping from inside a nested pair still takes the form before *it*.
        assert_eq!(run("(x (a b))", plan_slurp(&s("(x (a b))"), 5, false)), Some("((x a b))".into()));
        // Transpose swaps the neighbours of point.
        assert_eq!(run("a b c", plan_transpose(&s("a b c"), 1)), Some("b a c".into()));
        assert_eq!(run("a b c", plan_transpose(&s("a b c"), 3)), Some("a c b".into()));
        assert_eq!(run("a b c", plan_transpose(&s("a b c"), 4)), Some("a c b".into()), "in the gap or on the form");
        assert_eq!(run("(a b) (c d)", plan_transpose(&s("(a b) (c d)"), 6)), Some("(c d) (a b)".into()));
        // Wrapping a pair keeps it whole.
        let t = s("x (a b)");
        let span = form_at(&t, 4).unwrap();
        assert_eq!(run("x (a b)", Some(plan_wrap(&t, span, '[', ']'))), Some("x ([a] b)".into()));
        let t = s("(a b)");
        let span = form_at(&t, 2).unwrap();
        assert_eq!(span, (1, 2), "the word wins over the pair around it");
    }

    #[test]
    fn find_and_till_land_where_they_say() {
        let t = chars("a,b,c,d");
        // Forward from 0: f goes just past the comma, t stops before it.
        assert_eq!(find_target(&t, 0, ',', 1, false, true), Some(2));
        assert_eq!(find_target(&t, 0, ',', 1, false, false), Some(1));
        assert_eq!(find_target(&t, 0, ',', 2, false, true), Some(4), "the second one");
        // Backward from 6 ('d'): f lands on the comma (so the selection
        // includes it), t just after it.
        assert_eq!(find_target(&t, 6, ',', 1, true, true), Some(5));
        assert_eq!(find_target(&t, 6, ',', 1, true, false), Some(6));
        assert_eq!(find_target(&t, 6, ',', 2, true, true), Some(3));
        assert_eq!(find_target(&t, 6, ',', 4, true, true), None, "not that many");
        assert_eq!(find_target(&t, 0, 'z', 1, false, true), None);
        // meow-find-ref goes to the next / previous one, not the first.
        assert_eq!(find_ref(&t, 2, ',', false), Some(4));
        assert_eq!(find_ref(&t, 4, ',', true), Some(3));
        assert_eq!(find_ref(&t, 1, ',', true), None, "the comma at 1 is under point");
    }

    #[test]
    fn the_previous_match_is_the_last_one_before() {
        let line: Vec<char> = "ab ab ab".chars().collect();
        let at = |col: u32| Position { line: 0, column: col };
        let search = |from: Position| {
            (from.column as usize..line.len().saturating_sub(1))
                .find(|i| line[*i] == 'a' && line[i + 1] == 'b')
                .map(|i| Span { start: at(i as u32), end: at(i as u32 + 2) })
        };
        let step = |p: Position| at((p.column + 1).min(line.len() as u32));
        let col = |s: Option<Span>| s.map(|s| s.start.column);
        assert_eq!(col(previous_match(search, step, at(6))), Some(3), "before the last one");
        assert_eq!(col(previous_match(search, step, at(3))), Some(0));
        assert_eq!(col(previous_match(search, step, at(0))), None, "nothing before the first");
        assert_eq!(col(previous_match(search, step, at(8))), Some(6));
    }

    #[test]
    fn whole_line_kills_take_the_lines_touched() {
        let at = |line: u32, column: u32| Position { line, column };
        let len = |l: u32| [3, 4, 5][l as usize];
        let range = |r: Option<(Position, Position)>, line| {
            let (a, b) = whole_lines(r, line, 3, len);
            ((a.line, a.column), (b.line, b.column))
        };
        assert_eq!(range(None, 1), ((1, 0), (2, 0)), "a middle line, newline and all");
        assert_eq!(range(None, 2), ((2, 0), (2, 5)), "the last line has no newline");
        assert_eq!(range(Some((at(0, 1), at(1, 2))), 1), ((0, 0), (2, 0)), "every line touched");
        assert_eq!(range(Some((at(1, 2), at(0, 1))), 0).0, (1, 0), "callers pass them in order");
        assert_eq!(range(Some((at(0, 0), at(1, 0))), 1), ((0, 0), (1, 0)), "stops short of a line it only reaches");
    }

    #[test]
    fn swapping_two_stretches() {
        let s = chars;
        let t = s("aa-bb-cc");
        let (plan, a_now, b_now) = plan_swap(&t, (0, 2), (6, 8)).unwrap();
        assert_eq!(run("aa-bb-cc", Some(plan)), Some("cc-bb-aa".into()));
        assert_eq!((a_now, b_now), ((6, 8), (0, 2)));
        // Different lengths: the offsets say where each went.
        let t = s("x [one] y [three] z");
        let (plan, a_now, b_now) = plan_swap(&t, (3, 6), (11, 16)).unwrap();
        let out = run("x [one] y [three] z", Some(plan)).unwrap();
        assert_eq!(out, "x [three] y [one] z");
        let o = s(&out);
        assert_eq!(slice(&o, a_now.0, a_now.1), "one");
        assert_eq!(slice(&o, b_now.0, b_now.1), "three");
        // Order doesn't matter, overlap is refused.
        let (_, a_now, b_now) = plan_swap(&t, (11, 16), (3, 6)).unwrap();
        assert_eq!((a_now, b_now), ((3, 8), (13, 16)), "`three` is now first");
        assert!(plan_swap(&t, (0, 5), (3, 8)).is_none());
    }
}
