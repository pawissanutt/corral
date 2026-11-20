//! The app's design tokens: the colours of `theme.json` by role, the fonts in `assets/`, and the
//! text size and row height of each form.

use nsh_core::prelude::*;

use crate::model::{Form, Tone};

/// A colour role of `theme.json`. Shell roles colour the desktop title bar, the sidebar and the
/// phone health strip, which stay dark in both appearances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    Bg,
    BgSunk,
    Surface,
    Surface2,
    Shell,
    ShellInk,
    ShellInk2,
    ShellSel,
    /// The hairline between two parts of the shell.
    ShellRule,
    Rule,
    Rule2,
    Ink,
    Ink2,
    Ink3,
    Ink4,
    Sel,
    Hover,
    Focus,
    Btn,
    BtnInk,
    You,
    YouBg,
    Corral,
    CorralBg,
    Worker,
    WorkerBg,
    Held,
    HeldBg,
    Fail,
    FailBg,
    Ok,
    OkBg,
}

impl Token {
    /// Its key in `theme.json`. The mock's `bg`, `ink`, `btn` and `btn-ink` are shadcn's
    /// `background`, `foreground`, `primary` and `primary-foreground`, so components of `nsh_base`
    /// that read the shadcn names draw in the same colours.
    const fn key(self) -> &'static str {
        match self {
            Token::Bg => "background",
            Token::BgSunk => "bg-sunk",
            Token::Surface => "surface",
            Token::Surface2 => "surface-2",
            Token::Shell => "shell",
            Token::ShellInk => "shell-ink",
            Token::ShellInk2 => "shell-ink-2",
            Token::ShellSel => "shell-sel",
            Token::ShellRule => "shell-rule",
            Token::Rule => "rule",
            Token::Rule2 => "rule-2",
            Token::Ink => "foreground",
            Token::Ink2 => "ink-2",
            Token::Ink3 => "ink-3",
            Token::Ink4 => "ink-4",
            Token::Sel => "sel",
            Token::Hover => "hover",
            Token::Focus => "focus",
            Token::Btn => "primary",
            Token::BtnInk => "primary-foreground",
            Token::You => "you",
            Token::YouBg => "you-bg",
            Token::Corral => "corral",
            Token::CorralBg => "corral-bg",
            Token::Worker => "worker",
            Token::WorkerBg => "worker-bg",
            Token::Held => "held",
            Token::HeldBg => "held-bg",
            Token::Fail => "fail",
            Token::FailBg => "fail-bg",
            Token::Ok => "ok",
            Token::OkBg => "ok-bg",
        }
    }
}

/// The token's colour from the theme (light and dark values, switched natively). A token the theme
/// lacks falls back to the theme's foreground, so a typo shows instead of crashing.
pub fn color(token: Token) -> AdaptiveColor {
    let theme = nsh_base::theme();
    theme.color(token.key()).unwrap_or(theme.foreground)
}

pub const FONT_TEXT: &str = asset!("fonts/AtkinsonHyperlegibleNext-Regular.woff2").family;
pub const FONT_MONO: &str = asset!("fonts/AtkinsonHyperlegibleMono-Regular.woff2").family;
/// A pane's screen as read: Cascadia Mono, the product's terminal face (its regular, bold and
/// italic files are in `assets/fonts/`), so the screen looks the same for every owner whatever
/// their own terminal's font; `FONT_MONO` has no box drawing. Its box drawing, blocks, braille and
/// shapes take the letters' advance, so frames line up. A character it lacks falls to the
/// system's monospace: NSH maps `ui-monospace`/`monospace` to each platform's own, and on the web
/// `ui-monospace` is Safari's alone, so named families come before the generic one.
pub const FONT_SCREEN: &str =
    "Cascadia Mono, ui-monospace, Menlo, \"DejaVu Sans Mono\", Consolas, monospace";
const _: () = assert!(
    leads(
        FONT_SCREEN,
        asset!("fonts/CascadiaMono-Regular.woff2").family
    ),
    "FONT_SCREEN must start with the family the shipped Cascadia Mono file names"
);

/// Whether the family list `list` starts with `family`.
const fn leads(list: &str, family: &str) -> bool {
    let (list, family) = (list.as_bytes(), family.as_bytes());
    if list.len() < family.len() {
        return false;
    }
    let mut at = 0;
    while at < family.len() {
        if list[at] != family[at] {
            return false;
        }
        at += 1;
    }
    true
}

/// Text size and row height for the form: Desktop 13 / 30, Phone 16 / 56, Terminal 13 / 16 (points).
pub fn text_size(form: Form) -> f32 {
    match form {
        Form::Desktop | Form::Terminal => 13.0,
        Form::Phone => 16.0,
    }
}

pub fn row_height(form: Form) -> f32 {
    match form {
        Form::Desktop => 30.0,
        Form::Phone => 56.0,
        Form::Terminal => 16.0,
    }
}

/// The colour a health tone is drawn in.
pub fn tone(tone: Tone) -> AdaptiveColor {
    color(match tone {
        Tone::Ok => Token::Ok,
        Tone::Warn => Token::Held,
        Tone::Fail => Token::Fail,
        Tone::Muted => Token::Ink3,
    })
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
