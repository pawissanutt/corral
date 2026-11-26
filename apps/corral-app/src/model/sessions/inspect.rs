//! The Sessions inspector of the selected session: what waits on the owner first, then its
//! facts, the last rows of its transcript and its Details sections in brief. Only what the host
//! or corrald reports is listed; a fact neither reports is left out.

use crate::model::daemon::Feed;
use crate::model::effect::Env;
use crate::model::session::transcript::{self, Message, MessageItem, MessageKind};
use crate::model::session::{Cx, Facts, Ui, details_brief};
use crate::model::status::SessionState;
use crate::model::{Id, Live};

use super::row::{SessionRow, row};

mod mode;
pub use mode::{ModeBlock, changes_target, watch_changes, watch_host};

/// How many transcript rows the inspector shows.
const LATEST: usize = 4;

/// One transcript row in brief: who said it, and its first line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest {
    /// `you`, the agent's name, `tool`.
    pub who: String,
    pub words: String,
}

/// The inspector of one session.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspect {
    pub row: SessionRow,
    /// What it asks, while it waits on the owner; answered from the card under it.
    pub question: Option<String>,
    /// What it is doing, while it does not wait: `Working: editing panels/checkpoints.tsx`.
    pub state: String,
    /// `project`, `corral`; only the facts reported.
    pub facts: Vec<(&'static str, String)>,
    pub mode: ModeBlock,
    /// The run's task, whose page the task fact opens.
    pub task: Option<Id>,
    /// The newest transcript rows, oldest first; empty until the transcript is read.
    pub latest: Vec<Latest>,
    /// Each Details section's name and summary.
    pub details: Vec<(String, String)>,
}

impl Inspect {
    /// The page the inspector's Open page goes to.
    pub fn live(&self) -> Live {
        self.row.live()
    }
}

/// The first line of words a message item holds, when it holds words.
fn words_of(item: &MessageItem) -> Option<String> {
    let said = match item {
        MessageItem::Prose(prose) => prose.markdown.clone(),
        MessageItem::Paragraph(rich) => rich.iter().map(|span| span.text.as_str()).collect(),
        MessageItem::Tool(tool) => format!("{} {}", tool.name, tool.summary),
        MessageItem::Question(question) => question.question.clone(),
        _ => return None,
    };
    said.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

fn latest_of(message: &Message) -> Option<Latest> {
    if message.kind == MessageKind::Quiet {
        return None;
    }
    let words = message.items.iter().find_map(words_of)?;
    let who = match message.kind {
        MessageKind::Bubble => "you".to_owned(),
        _ if message.author.is_empty() => "agent".to_owned(),
        _ => message.author.clone(),
    };
    Some(Latest { who, words })
}

/// The newest rows of a transcript the app has read, oldest first.
pub fn latest(messages: &[Message]) -> Vec<Latest> {
    let mut rows: Vec<Latest> = messages
        .iter()
        .rev()
        .filter_map(latest_of)
        .take(LATEST)
        .collect();
    rows.reverse();
    rows
}

/// The inspector of session `id`; none when the daemon lists no such session.
pub fn inspect(feed: &Feed, id: &Id, env: Env) -> Option<Inspect> {
    let row = row(feed, id, env)?;
    let facts = Facts::of(feed, id);
    let ui = Ui::default();
    let cx = Cx::new(feed, &ui, id, env, &facts);
    let shown = transcript::transcript(id, &ui, cx.cache, feed);
    let messages: Vec<Message> = shown.messages.iter().cloned().collect();
    let blocked = row.line.state == SessionState::Blocked;
    let state = match &row.line.doing {
        Some(doing) if !blocked => format!("{}: {doing}", row.line.state.word()),
        _ => row.line.state.word().to_owned(),
    };
    let mut listed: Vec<(&'static str, String)> = vec![("role", row.role.clone())];
    listed.extend(row.line.project.clone().map(|project| ("project", project)));
    if cx.build().is_none() {
        listed.extend(row.branch.clone().map(|branch| ("branch", branch)));
        listed.extend(row.context.clone().map(|context| ("context", context)));
    }
    listed.extend(row.model_effort().map(|model| ("model", model)));
    listed.extend(row.cost.clone().map(|cost| ("cost", cost)));
    listed.extend(row.age.clone().map(|age| ("age", age)));
    Some(Inspect {
        question: blocked.then(|| row.line.doing.clone()).flatten(),
        state,
        facts: listed,
        mode: mode::preview(&cx, &messages),
        task: cx.task.cloned(),
        latest: latest(&messages),
        details: details_brief(&cx),
        row,
    })
}
