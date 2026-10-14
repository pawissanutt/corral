//! ratatui UI for corral. UX reference: docs/mockup/corral-tui.html (open in a browser; its scenes and
//! keymap are the spec). Screens are `View`s; `app` routes between them.

pub mod app;
pub mod artifacts;
pub mod board;
pub mod detail;
pub mod overlays;
pub mod review;
pub mod theme;

use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

pub use app::run;

/// Which UI to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Full board (`corral board`).
    Board,
    /// Capture popup only; exits after saving or Esc (`corral capture`). The path is the directory the user
    /// was working in (used to infer the project); the CLI takes it from the herdr plugin context's focused pane cwd.
    Capture(std::path::PathBuf),
    /// Artifact finder only; exits on Esc (`corral artifacts`).
    Artifacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Warn,
    Bad,
    Attention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub title: String,
    pub body: String,
    pub tone: Tone,
}

impl Toast {
    pub fn new(tone: Tone, title: impl Into<String>, body: impl Into<String>) -> Toast {
        Toast {
            title: title.into(),
            body: body.into(),
            tone,
        }
    }
}

/// What a view asks the app router to do after handling a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Key handled (or ignored); just redraw.
    None,
    /// Close this view / overlay and return to the previous one.
    Back,
    /// Exit the program.
    Quit,
    /// Push the task detail view.
    OpenDetail(String),
    /// Push the review view.
    OpenReview(String),
    /// Push the artifact finder.
    OpenArtifacts,
    /// Focus the run's herdr pane (app calls `corral_ops::jump`).
    Jump(String),
    /// Show a toast (app keeps it ~4 s).
    Toast(Toast),
    /// Show a toast, then close this view.
    BackWithToast(Toast),
    /// Re-read the store (and run sync) now.
    Reload,
}

/// Every full-screen view and overlay implements this. `draw` must not do I/O beyond cheap store reads;
/// views cache what they load and refresh on `refresh`.
pub trait View {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx);
    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action;
    /// Footer key hints, e.g. [("⏎", "open"), ("esc", "back")].
    fn hints(&self) -> Vec<(&'static str, &'static str)>;
    /// Called by the app after Reload / periodic sync so the view can re-read data. Default: nothing.
    fn refresh(&mut self, _ctx: &Ctx) {}
    /// True for overlays drawn on top of the previous view (popups). Default: false (full screen).
    fn is_overlay(&self) -> bool {
        false
    }
    /// True while the view has a focused text input, so the app must not treat letters as global keys.
    fn captures_text(&self) -> bool {
        false
    }
}
