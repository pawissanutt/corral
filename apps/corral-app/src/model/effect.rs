//! What a reducer asks the app to do, and the switches every screen reads: the connection state,
//! the data state, and the device the app runs on.

use crate::model::nav::{Destination, Form, SettingsSection};

/// The connection states of the design (its "Connection" switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Conn {
    #[default]
    Healthy,
    Degraded,
    Down,
    Relay,
    Offline,
}

impl Conn {
    pub const ALL: [Conn; 5] = [
        Conn::Healthy,
        Conn::Degraded,
        Conn::Down,
        Conn::Relay,
        Conn::Offline,
    ];

    /// The storage slug, also the name the design's deep links use.
    pub const fn slug(self) -> &'static str {
        match self {
            Conn::Healthy => "healthy",
            Conn::Degraded => "degraded",
            Conn::Down => "down",
            Conn::Relay => "relay",
            Conn::Offline => "offline",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Conn> {
        Conn::ALL.into_iter().find(|conn| conn.slug() == slug)
    }

    /// Commands that need a live connection refuse in these states; nothing is queued.
    pub const fn refuses(self) -> bool {
        matches!(self, Conn::Offline | Conn::Down)
    }
}

/// The data states of the design (its "Data" switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DataMode {
    #[default]
    Live,
    /// The projection arrived, detail has not.
    Loading,
    Empty,
    Error,
    /// The question was answered on another surface.
    Elsewhere,
}

impl DataMode {
    pub const ALL: [DataMode; 5] = [
        DataMode::Live,
        DataMode::Loading,
        DataMode::Empty,
        DataMode::Error,
        DataMode::Elsewhere,
    ];

    pub const fn slug(self) -> &'static str {
        match self {
            DataMode::Live => "live",
            DataMode::Loading => "loading",
            DataMode::Empty => "empty",
            DataMode::Error => "error",
            DataMode::Elsewhere => "elsewhere",
        }
    }

    pub fn from_slug(slug: &str) -> Option<DataMode> {
        DataMode::ALL.into_iter().find(|mode| mode.slug() == slug)
    }
}

/// What every view builder and reducer may read besides its own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    pub conn: Conn,
    pub mode: DataMode,
    pub form: Form,
}

impl Env {
    /// The device the app runs on, as attribution names it.
    pub const fn device(self) -> &'static str {
        match self.form {
            Form::Phone => "pawis-iphone",
            Form::Desktop | Form::Terminal => "pawis-desktop",
        }
    }

    /// `via=app:<device>`.
    pub fn via(self) -> String {
        format!("via=app:{}", self.device())
    }
}

/// A page above a destination. Ids are fixture ids, so they are static.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    /// The handover sheet of question T-868/R1/Q1, in focus mode.
    Question,
    /// A task: selected in Now on desktop and terminal, a pushed page on phone.
    Task(&'static str),
    /// A plan: selected in Plans on desktop and terminal, a pushed page on phone.
    Plan(&'static str),
    /// An agent's session, by task id or "PM".
    Session(&'static str),
    /// An assistant thread, by session id ("A-7" is the quick thread).
    Thread(&'static str),
}

impl Page {
    /// The destination the page belongs to.
    pub const fn owner(self) -> Destination {
        match self {
            Page::Question => Destination::Inbox,
            Page::Task(_) => Destination::Now,
            Page::Plan(_) => Destination::Plans,
            Page::Session(_) | Page::Thread(_) => Destination::Agents,
        }
    }
}

/// A command that waits for a fresh confirmation on this device, with a preview of its exact
/// effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Risk {
    AcceptPlan,
    ApproveReview,
    Revoke,
    ModePause,
    Discard,
}

/// What a reducer asks the app to do after it changed its own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Show the text for a few seconds.
    Toast(String),
    Go(Destination),
    GoSettings(SettingsSection),
    Open(Page),
    /// Leave the current page for the one below it.
    Back,
    /// Ask for a confirmation of the command, naming what it applies to when the title alone
    /// does not (a device to revoke, an idea to discard).
    Confirm(Risk, Option<String>),
    /// Open Ask, attaching a context chip and setting the draft when given.
    Ask {
        chip: Option<&'static str>,
        text: Option<String>,
    },
    CloseAsk,
    /// Switch the connection state (the Connection settings' state cards do).
    SetConn(Conn),
}
