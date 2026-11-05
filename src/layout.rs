// SPDX-License-Identifier: GPL-3.0-or-later
//! The keyboard layouts Meow ships with.
//!
//! Meow's own documentation shows a layer for each layout it knows, and
//! these are those layers: the same keys, the same commands. Switching is
//! one command away (`meow-setup-qwerty`, `meow-setup-qwertz`,
//! `meow-setup-dvorak`, `meow-setup-colemak`), and the QWERTZ layout
//! brings its own thing keys with it, as Meow's does.
//!
//! Two keys are the plugin's own rather than Meow's, and are said so in
//! the README: `SPC` opens the keypad, and `:` is `execute-extended-command`.

/// One layout: the keys of the normal state, and the things they name.
pub struct Layout {
    /// The name, as the message shows it.
    pub name: &'static str,
    /// The normal-state keys.
    pub normal: &'static [(&'static str, &'static str)],
    /// The motion-state keys, for the layouts that add some.
    pub motion: &'static [(&'static str, &'static str)],
    /// The thing keys, for the layouts that have their own.
    pub things: &'static [(&'static char, &'static str)],
}

/// The digits, which every layout has the same way.
const EXPANDS: [(&str, &str); 10] = [
    ("0", "meow-expand-0"),
    ("1", "meow-expand-1"),
    ("2", "meow-expand-2"),
    ("3", "meow-expand-3"),
    ("4", "meow-expand-4"),
    ("5", "meow-expand-5"),
    ("6", "meow-expand-6"),
    ("7", "meow-expand-7"),
    ("8", "meow-expand-8"),
    ("9", "meow-expand-9"),
];

/// Meow's QWERTY layer, from `KEYBINDING_QWERTY.org`.
const QWERTY: Layout = Layout {
    name: "qwerty",
    normal: &[
        ("-", "negative-argument"),
        (";", "meow-reverse"),
        (",", "meow-inner-of-thing"),
        (".", "meow-bounds-of-thing"),
        ("[", "meow-beginning-of-thing"),
        ("]", "meow-end-of-thing"),
        ("a", "meow-append"),
        ("A", "meow-open-below"),
        ("b", "meow-back-word"),
        ("B", "meow-back-symbol"),
        ("c", "meow-change"),
        ("d", "meow-delete"),
        ("D", "meow-backward-delete"),
        ("e", "meow-next-word"),
        ("E", "meow-next-symbol"),
        ("f", "meow-find"),
        ("g", "meow-cancel-selection"),
        ("G", "meow-grab"),
        ("h", "meow-left"),
        ("H", "meow-left-expand"),
        ("i", "meow-insert"),
        ("I", "meow-open-above"),
        ("j", "meow-next"),
        ("J", "meow-next-expand"),
        ("k", "meow-prev"),
        ("K", "meow-prev-expand"),
        ("l", "meow-right"),
        ("L", "meow-right-expand"),
        ("m", "meow-join"),
        ("n", "meow-search"),
        ("o", "meow-block"),
        ("O", "meow-to-block"),
        ("p", "meow-yank"),
        ("q", "meow-quit"),
        ("Q", "meow-goto-line"),
        ("r", "meow-replace"),
        ("R", "meow-swap-grab"),
        ("s", "meow-kill"),
        ("t", "meow-till"),
        ("u", "meow-undo"),
        ("U", "meow-undo-in-selection"),
        ("v", "meow-visit"),
        ("w", "meow-mark-word"),
        ("W", "meow-mark-symbol"),
        ("x", "meow-line"),
        ("X", "meow-goto-line"),
        ("y", "meow-save"),
        ("Y", "meow-sync-grab"),
        ("z", "meow-pop-selection"),
        ("'", "meow-repeat-last-key"),
        ("/", "meow-search"),
    ],
    motion: &[],
    things: &[],
};

/// Meow's QWERTZ layer, from `KEYBINDING_QWERTZ.org`. The things it names
/// are Meow's too: `f` is a round bracket there, not here.
const QWERTZ: Layout = Layout {
    name: "qwertz",
    normal: &[
        ("ä", "meow-reverse"),
        ("i", "meow-prev"),
        ("k", "meow-next"),
        ("j", "meow-left"),
        ("l", "meow-right"),
        ("z", "meow-search"),
        ("-", "meow-visit"),
        ("I", "meow-prev-expand"),
        ("K", "meow-next-expand"),
        ("J", "meow-left-expand"),
        ("L", "meow-right-expand"),
        ("u", "meow-back-word"),
        ("U", "meow-back-symbol"),
        ("o", "meow-next-word"),
        ("O", "meow-next-symbol"),
        ("a", "meow-mark-word"),
        ("A", "meow-mark-symbol"),
        ("s", "meow-line"),
        ("S", "meow-goto-line"),
        ("w", "meow-block"),
        ("q", "meow-join"),
        ("g", "meow-grab"),
        ("G", "meow-pop-grab"),
        ("m", "meow-swap-grab"),
        ("M", "meow-sync-grab"),
        ("p", "meow-cancel-selection"),
        ("P", "meow-pop-selection"),
        ("x", "meow-till"),
        ("y", "meow-find"),
        (",", "meow-beginning-of-thing"),
        (".", "meow-end-of-thing"),
        (";", "meow-inner-of-thing"),
        (":", "meow-bounds-of-thing"),
        ("d", "meow-kill"),
        ("f", "meow-change"),
        ("t", "meow-delete"),
        ("c", "meow-save"),
        ("v", "meow-yank"),
        ("V", "meow-yank-pop"),
        ("e", "meow-insert"),
        ("E", "meow-open-above"),
        ("r", "meow-append"),
        ("R", "meow-open-below"),
        ("h", "meow-undo"),
        ("H", "undo-redo"),
        ("b", "open-line"),
        ("B", "newline"),
    ],
    motion: &[],
    things: &[
        (&'f', "round"),
        (&'d', "square"),
        (&'s', "curly"),
        (&'a', "angle"),
        (&'r', "string"),
        (&'v', "paragraph"),
        (&'c', "line"),
        (&'x', "buffer"),
    ],
};

/// Meow's Dvorak layer, from `KEYBINDING_DVORAK.org`.
const DVORAK: Layout = Layout {
    name: "dvorak",
    normal: &[
        ("-", "negative-argument"),
        (";", "meow-reverse"),
        (",", "meow-inner-of-thing"),
        (".", "meow-bounds-of-thing"),
        ("<", "meow-beginning-of-thing"),
        (">", "meow-end-of-thing"),
        ("a", "meow-append"),
        ("A", "meow-open-below"),
        ("b", "meow-back-word"),
        ("B", "meow-back-symbol"),
        ("c", "meow-change"),
        ("d", "meow-delete"),
        ("D", "meow-backward-delete"),
        ("e", "meow-line"),
        ("E", "meow-goto-line"),
        ("f", "meow-find"),
        ("g", "meow-cancel-selection"),
        ("G", "meow-grab"),
        ("h", "meow-left"),
        ("H", "meow-left-expand"),
        ("i", "meow-insert"),
        ("I", "meow-open-above"),
        ("j", "meow-join"),
        ("k", "meow-kill"),
        ("l", "meow-till"),
        ("m", "meow-mark-word"),
        ("M", "meow-mark-symbol"),
        ("n", "meow-next"),
        ("N", "meow-next-expand"),
        ("o", "meow-block"),
        ("O", "meow-to-block"),
        ("p", "meow-prev"),
        ("P", "meow-prev-expand"),
        ("q", "meow-quit"),
        ("Q", "meow-goto-line"),
        ("r", "meow-replace"),
        ("R", "meow-swap-grab"),
        ("s", "meow-search"),
        ("t", "meow-right"),
        ("T", "meow-right-expand"),
        ("u", "meow-undo"),
        ("U", "meow-undo-in-selection"),
        ("v", "meow-visit"),
        ("w", "meow-next-word"),
        ("W", "meow-next-symbol"),
        ("x", "meow-save"),
        ("X", "meow-sync-grab"),
        ("y", "meow-yank"),
        ("z", "meow-pop-selection"),
        ("'", "meow-repeat-last-key"),
    ],
    motion: &[],
    things: &[],
};

/// Meow's Colemak layer, from `KEYBINDING_COLEMAK.org`.
const COLEMAK: Layout = Layout {
    name: "colemak",
    normal: &[
        ("-", "negative-argument"),
        (";", "meow-reverse"),
        (",", "meow-inner-of-thing"),
        (".", "meow-bounds-of-thing"),
        ("[", "meow-beginning-of-thing"),
        ("]", "meow-end-of-thing"),
        ("/", "meow-visit"),
        ("a", "meow-append"),
        ("A", "meow-open-below"),
        ("b", "meow-back-word"),
        ("B", "meow-back-symbol"),
        ("c", "meow-change"),
        ("e", "meow-prev"),
        ("E", "meow-prev-expand"),
        ("f", "meow-find"),
        ("g", "meow-cancel-selection"),
        ("G", "meow-grab"),
        ("h", "meow-left"),
        ("H", "meow-left-expand"),
        ("i", "meow-right"),
        ("I", "meow-right-expand"),
        ("j", "meow-join"),
        ("k", "meow-kill"),
        ("l", "meow-line"),
        ("L", "meow-goto-line"),
        ("m", "meow-mark-word"),
        ("M", "meow-mark-symbol"),
        ("n", "meow-next"),
        ("N", "meow-next-expand"),
        ("o", "meow-block"),
        ("O", "meow-to-block"),
        ("p", "meow-yank"),
        ("q", "meow-quit"),
        ("r", "meow-replace"),
        ("s", "meow-insert"),
        ("S", "meow-open-above"),
        ("t", "meow-till"),
        ("u", "meow-undo"),
        ("U", "meow-undo-in-selection"),
        ("v", "meow-search"),
        ("w", "meow-next-word"),
        ("W", "meow-next-symbol"),
        ("x", "meow-delete"),
        ("X", "meow-backward-delete"),
        ("y", "meow-save"),
        ("z", "meow-pop-selection"),
        ("'", "meow-repeat-last-key"),
    ],
    motion: &[("e", "meow-prev")],
    things: &[],
};

/// Every layout, in the order the commands are shown in.
pub const LAYOUTS: &[&Layout] = &[&QWERTY, &QWERTZ, &DVORAK, &COLEMAK];

/// The layout called `name`.
pub fn layout(name: &str) -> Option<&'static Layout> {
    LAYOUTS.iter().copied().find(|l| l.name == name)
}

/// The names of the layouts, for the message that lists them.
pub fn names() -> Vec<&'static str> {
    LAYOUTS.iter().map(|l| l.name).collect()
}

/// The normal-state keys of a layout, digits and all.
pub fn normal_keys(layout: &Layout) -> Vec<(&'static str, &'static str)> {
    let mut keys: Vec<(&'static str, &'static str)> = EXPANDS.to_vec();
    keys.extend_from_slice(layout.normal);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layouts_are_the_ones_meow_shows() {
        assert_eq!(names(), vec!["qwerty", "qwertz", "dvorak", "colemak"]);
        let qwerty = layout("qwerty").unwrap();
        assert!(normal_keys(qwerty).contains(&("s", "meow-kill")));
        let qwertz = layout("qwertz").unwrap();
        assert!(normal_keys(qwertz).contains(&("d", "meow-kill")));
        assert_eq!(qwertz.things[0], (&'f', "round"));
        let dvorak = layout("dvorak").unwrap();
        assert!(normal_keys(dvorak).contains(&("k", "meow-kill")));
        let colemak = layout("colemak").unwrap();
        assert!(normal_keys(colemak).contains(&("k", "meow-kill")));
    }

    #[test]
    fn no_layout_binds_a_key_twice() {
        for layout in LAYOUTS {
            let keys = normal_keys(layout);
            let mut seen: Vec<&str> = Vec::new();
            for (key, _) in &keys {
                assert!(!seen.contains(key), "{} binds {key} twice", layout.name);
                seen.push(key);
            }
        }
    }
}
