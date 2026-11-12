//! Text with the few inline styles the design uses, so the model can carry a sentence whole:
//! bold names, mono ids and commands, dim asides, struck verdicts, state colour, and links into
//! the app.

use crate::model::effect::Page;
use crate::model::hue::Hue;

/// One run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub mono: bool,
    pub dim: bool,
    pub strike: bool,
    pub hue: Hue,
    /// Where pressing the run leads.
    pub link: Option<Page>,
}

/// A sentence or paragraph: its runs in order, with no separator added between them.
pub type Rich = Vec<Span>;

/// A plain run.
pub fn span(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        ..Span::default()
    }
}

impl Span {
    pub fn bold(mut self) -> Span {
        self.bold = true;
        self
    }

    pub fn mono(mut self) -> Span {
        self.mono = true;
        self
    }

    pub fn dim(mut self) -> Span {
        self.dim = true;
        self
    }

    pub fn strike(mut self) -> Span {
        self.strike = true;
        self
    }

    pub fn hue(mut self, hue: Hue) -> Span {
        self.hue = hue;
        self
    }

    pub fn link(mut self, page: Page) -> Span {
        self.link = Some(page);
        self
    }
}

/// A task id as the design links it: mono, opening the task.
pub fn task_link(id: &'static str) -> Span {
    span(id).mono().link(Page::Task(id))
}

/// The text without its styles, for a terminal line, an accessibility label or a search.
pub fn flat(rich: &[Span]) -> String {
    rich.iter().map(|span| span.text.as_str()).collect()
}
