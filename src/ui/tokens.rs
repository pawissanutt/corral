//! The app's design tokens: the colours of `theme.json` by role, the two fonts in `assets/`, and the
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
