//! Shared terminal-tail and inspector descriptors.

use super::Press;

/// The terminal syntax classes encoded by the mockup's tail helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailTone {
    /// Unstyled output.
    Plain,
    /// A shell command or Edit/Bash/Read tool line.
    Command,
    /// A passing test or `ok ` line.
    Ok,
}

impl TailTone {
    /// The tone of a terminal line, as the mockup's tail helper decides it.
    pub fn of(line: &str) -> TailTone {
        if line.contains("PASS") || line.contains("ok ") {
            TailTone::Ok
        } else if ["$", "Edit", "Bash", "Read"]
            .iter()
            .any(|prefix| line.starts_with(*prefix))
        {
            TailTone::Command
        } else {
            TailTone::Plain
        }
    }
}

/// A terminal tail line, with text unmodified and styling decided in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailLine {
    /// Exact terminal text, including indentation.
    pub text: String,
    /// Semantic style for this line.
    pub tone: TailTone,
}

impl TailLine {
    pub fn new(text: impl Into<String>) -> TailLine {
        let text = text.into();
        TailLine {
            tone: TailTone::of(&text),
            text,
        }
    }
}

/// A task's terminal tail: a live stream in the fixture, a read-only capture from corrald.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tail {
    /// Task or PM identity.
    pub id: String,
    /// Pane at the top of the tail; `−` if absent.
    pub pane: String,
    /// The runtime name.
    pub runtime: String,
    /// `live`, or a capture's label with its time.
    pub label: String,
    /// Whether this tail is a capture rather than a stream.
    pub snapshot: bool,
    /// Attribution shown above the terminal, in the fixture preview.
    pub illustrative: Option<&'static str>,
    /// Terminal output in source order.
    pub lines: Vec<TailLine>,
    /// Draw the source's final cursor cell after the output: only a live stream has one.
    pub cursor: bool,
    /// Open the full session for this tail.
    pub action: Option<Press>,
}
/// Which selected-inspector content the renderer should show.
#[derive(Debug, Clone, PartialEq)]
pub enum Inspector {
    /// Projection detail is still loading; show three placeholders.
    Loading,
    /// The selected daemon task's story.
    LiveStory(Box<super::LiveStory>),
    /// The selected daemon task's artifacts.
    LiveEvidence(super::LiveEvidence),
    /// The selected daemon task's run and log tail.
    LiveSession(super::LiveSession),
    /// One quiet line in place of content: no board, nothing on it, an unenrolled device.
    Quiet(&'static str),
}

/// One delivery stage of the selected task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub label: &'static str,
    pub mark: crate::model::board::Mark,
    pub word: &'static str,
    pub detail: String,
    pub active: bool,
    pub hue: crate::model::hue::Hue,
}

/// A dependency and the route to its task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub id: String,
    pub mark: crate::model::board::Mark,
    pub show_mark: bool,
    pub action: Press,
}

/// Recorded worker progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub label: &'static str,
    pub done: usize,
    pub total: usize,
    pub figure: String,
}

/// The latest recorded state of a running agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTile {
    pub id: String,
    pub role: String,
    pub doing: String,
    pub doing_hue: crate::model::hue::Hue,
    pub profile: String,
    pub runtime: String,
    pub pane: String,
    pub chip: &'static str,
    pub chip_hue: crate::model::hue::Hue,
    pub title: String,
    pub since: String,
    pub tail: Tail,
    pub action: Option<Press>,
}
