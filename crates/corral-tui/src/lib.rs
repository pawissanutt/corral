//! ratatui UI for corral. UX reference: docs/mockup/corral-tui.html (open in a browser; its scenes and
//! keymap are the spec). Screens are `View`s; `app` routes between them.

pub mod app;
pub mod artifacts;
pub mod board;
pub(crate) mod choice;
pub mod detail;
pub mod discovery;
pub(crate) mod guide;
pub mod history;
pub mod hit;
pub mod nav;
pub(crate) mod obligations;
pub mod overlays;
pub mod review;
pub mod settings;
pub(crate) mod shelf;
pub mod term;
pub mod theme;

use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;

pub use app::run;
pub use discovery::{HarnessLoader, HarnessMsg};
pub use hit::{Hit, HitMap, MouseCtx};
pub use term::mouse_wanted;

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
    /// Settings screen only; exits on Esc (`corral settings`).
    Settings,
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
    /// The task this toast is about; clicking the toast opens it (detail, or review for a Review card).
    pub task: Option<String>,
}

impl Toast {
    pub fn new(tone: Tone, title: impl Into<String>, body: impl Into<String>) -> Toast {
        Toast {
            title: title.into(),
            body: body.into(),
            tone,
            task: None,
        }
    }

    /// Marks the toast as being about `task`, so a click on it opens that task.
    pub fn about(mut self, task: impl Into<String>) -> Toast {
        self.task = Some(task.into());
        self
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
    /// Push the task detail view on its Acceptance tab.
    OpenAcceptance(String),
    /// Push the artifact finder.
    OpenArtifacts,
    /// Push the history view (finished and dropped tasks).
    OpenHistory,
    /// Push the settings screen.
    OpenSettings,
    /// Open the pin at this 0-based shelf position through the viewer layer (`shelf::open`).
    OpenPin(usize),
    /// Focus the run's herdr pane (app calls `corral_ops::jump`).
    Jump(String),
    /// Show a toast (app keeps it ~4 s).
    Toast(Toast),
    /// Show a toast, then close this view.
    BackWithToast(Toast),
    /// Re-read the store (and run sync) now.
    Reload,
    /// Flip the review autopilot kill switch (`[review] autopilot`) and sync.
    ToggleAutopilot,
    /// Show the welcome tour (`t` on the board or in help).
    OpenTour,
}

/// Every full-screen view and overlay implements this. `draw` must not do I/O beyond cheap store reads;
/// views cache what they load and refresh on `refresh`. While drawing, views push their clickable regions into
/// `hits` (docs/DESIGN.md §10 "Input: keyboard and mouse").
pub trait View {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap);
    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action;
    /// A mouse event the app did not resolve itself (it handles `Hit::Key`, `Hit::Dismiss` and `Hit::Inert`).
    /// Must produce the same `Action` as the matching key. Default: ignore.
    fn on_mouse(&mut self, _ev: MouseEvent, _m: &MouseCtx) -> Action {
        Action::None
    }
    /// True while a popup holds typed text a stray outside click must not throw away. Default: false.
    fn has_unsaved_text(&self) -> bool {
        false
    }
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
