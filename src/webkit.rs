// SPDX-License-Identifier: GPL-3.0-or-later
//! Vim-style keys for web pages: Vim's way of browsing, on a page. Scroll
//! with `j` and `k`, label every link and type the label (`f`), `H` and `L`
//! for history, `x` to close a tab. Forge's
//! web pages (`M-x webkit`) are buffers, so those keys are a layer of Meow's
//! motion state, active in `webkit-mode` buffers only.
//!
//! In a web page the layer *is* motion state: the buffer has this one minor
//! mode instead of `meow-motion-mode` (same `[M]` indicator, and the keypad on
//! `SPC` and any `meow-motion-define-key` bindings carry over), so there is
//! one state, not two stacked.
//!
//! The layer sits above the page's own keys (Emacs's xwidget-webkit map: `n`,
//! `p`, `b`, `f`, ...), so where the two disagree these keys win; a key this
//! layer does not bind still runs the page's command. Typing into a page is
//! `i` (or `gi` for its first text field); Escape gives the keys back.
//!
//! `meow-webkit-keys` (a setting, and the command of the same name) turns it
//! off, leaving motion state's plain page keys.
//!
//! Left out, because Forge has no such thing or Meow already has its own:
//! tab pinning and moving, bookmarks and quick-open search engines,
//! marks (`m`, `'`), per-site blacklists, custom JS commands, the
//! open-link-in-background modifiers, and Vim's insert/normal switch (Meow
//! has its own states). Counts work: `5j` scrolls five lines.

use super::*;

pub const MODE: &str = "meow-webkit-mode";
const WEBKIT_MODE: &str = "webkit-mode";

/// The default map, as Forge commands. `g`, `y`, `z`, `[` and `]` are
/// prefixes.
pub const KEYS: &[(&str, &str)] = &[
    // Scrolling.
    ("j", "webkit-scroll-up-line"),
    ("k", "webkit-scroll-down-line"),
    ("h", "webkit-scroll-backward"),
    ("l", "webkit-scroll-forward"),
    ("d", "webkit-scroll-half-up"),
    ("u", "webkit-scroll-half-down"),
    ("g g", "webkit-scroll-top"),
    ("G", "webkit-scroll-bottom"),
    // Following links.
    ("f", "webkit-hints"),
    ("F", "webkit-hints-new-page"),
    ("y f", "webkit-hints-copy"),
    ("] ]", "webkit-next-link"),
    ("[ [", "webkit-previous-link"),
    // Going places.
    ("o", "webkit-browse-url"),
    ("O", "webkit-new"),
    ("p", "webkit-open-clipboard"),
    ("P", "webkit-open-clipboard-new"),
    ("H", "webkit-back"),
    ("L", "webkit-forward"),
    ("g h", "webkit-browse-history"),
    ("g u", "webkit-up-url"),
    ("g U", "webkit-root-url"),
    ("r", "webkit-reload"),
    // Pages (tabs).
    ("t", "webkit-new"),
    ("x", "webkit-quit"),
    ("X", "webkit-reopen-closed"),
    ("J", "webkit-previous-page"),
    ("K", "webkit-next-page"),
    ("T", "webkit-list-pages"),
    // Finding, copying, typing.
    ("/", "webkit-find"),
    ("n", "webkit-find-next"),
    ("N", "webkit-find-previous"),
    ("y y", "webkit-copy-url"),
    ("y s", "webkit-copy-selection"),
    ("i", "webkit-focus"),
    ("g i", "webkit-focus-input"),
    // Zoom.
    ("z i", "webkit-zoom-in"),
    ("z o", "webkit-zoom-out"),
    ("z z", "webkit-zoom-reset"),
];

thread_local! {
    static ON: Cell<bool> = const { Cell::new(true) };
}

/// Is the layer wanted?
pub fn enabled() -> bool {
    ON.with(Cell::get)
}

/// Does this web page get the layer in place of plain motion state?
pub fn applies(major_mode: &str) -> bool {
    enabled() && major_mode == WEBKIT_MODE
}

pub fn init() -> Result<(), String> {
    ON.with(|on| on.set(config::with(|o| o.webkit_keys)));
    modes::define_minor_mode(MODE)?;
    modes::set_lighter(MODE, "[M]")?;
    let mut skipped = Vec::new();
    for (key, command) in KEYS {
        if let Err(e) = bind_in_mode(MODE, key, command) {
            skipped.push(format!("{key} ({e})"));
        }
    }
    // 1-9 start a count: `5j` is five lines.
    for d in '1'..='9' {
        if let Err(e) = bind_in_mode(MODE, &d.to_string(), "digit-argument") {
            skipped.push(format!("{d} ({e})"));
        }
    }
    if !skipped.is_empty() {
        message(&format!("meow: web page keys left out: {}", skipped.join(", ")));
    }
    command(
        "meow-webkit-keys",
        "Turn the Vim-style keys for web pages on or off.\n\nThey are j k h l d u gg G (scroll), f F yf (label links, then type one), [[ ]] (previous / next link), H L (history), o O p P (open), t x X J K T (pages), / n N (find), yy (copy the address), i gi (type into the page), zi zo zz (zoom).",
        |_| {
            let on = !enabled();
            ON.with(|s| s.set(on));
            for b in Buffer::all() {
                let modes = b.minor_modes();
                if modes.contains(&MOTION.to_string()) || modes.contains(&MODE.to_string()) {
                    motion(&b);
                }
            }
            message(&format!(
                "Vim-style keys for web pages {}",
                if on { "enabled" } else { "disabled" }
            ));
            Ok(())
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_bound_once_and_names_a_webkit_command() {
        let mut seen = std::collections::HashSet::new();
        for (key, command) in KEYS {
            assert!(seen.insert(*key), "{key} is bound twice");
            assert!(command.starts_with("webkit-"), "{command}");
        }
    }

    #[test]
    fn prefixes_do_not_shadow_keys() {
        // A key that is also the start of a longer one could never fire.
        for (a, _) in KEYS {
            for (b, _) in KEYS {
                assert!(
                    a == b || !b.starts_with(&format!("{a} ")),
                    "{a} is a prefix of {b}"
                );
            }
        }
    }

    #[test]
    fn the_layer_only_applies_to_web_pages() {
        ON.with(|s| s.set(true));
        assert!(applies("webkit-mode"));
        assert!(!applies("help-mode"));
        ON.with(|s| s.set(false));
        assert!(!applies("webkit-mode"));
        ON.with(|s| s.set(true));
    }
}
