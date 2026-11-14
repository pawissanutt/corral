//! Inbox list descriptors, and the fixture preview's rows, changes feed and selection panes.
//! Answered rows stay in place; their displayed count intentionally differs from the
//! navigation's needs-you badge.

use super::sheet::button;
use super::{
    Action, Button, ButtonKind, Chip, Notice, Q, QuestionState, Selection, Sheet, State,
    fixture_desk, phone,
};
use crate::model::Id;
use crate::model::board::{self, ChangeKind};
use crate::model::effect::{DataMode, Env, Page};
use crate::model::hue::Hue;
use crate::model::nav::Destination;
use crate::model::text::{Rich, span};

/// One needs-you row or phone card, with every visible string resolved for the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedYouItem {
    /// The row's test id: `inbox-<key>` for served rows, the fixture's own names otherwise.
    pub test_id: String,
    /// Lead phrase, also available separately for accessibility.
    pub lead: String,
    /// Record id.
    pub id: String,
    /// Complete visible lead line, including sample attribution where appropriate.
    pub heading: Rich,
    /// Main title, with the phone plan prefix already included.
    pub title: String,
    /// Full resolved secondary text.
    pub meta: String,
    /// Right-side desktop age; phone ages are already included in heading.
    pub age: String,
    /// Question state, absent on other row kinds.
    pub state: Option<QuestionState>,
    /// Neutralises the owner dot when the item is no longer open.
    pub hue: Hue,
    /// Desktop selection state.
    pub selected: bool,
    /// Explicit sample label, absent for real rows.
    pub sample: Option<&'static str>,
    /// The desktop select or phone open action.
    pub action: Action,
}

/// A changes-feed row: time, id, kind and text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRow {
    /// Local time of the change.
    pub at: String,
    /// The changed task, or empty for workspace-wide changes.
    pub id: String,
    /// The mark shape.
    pub kind: ChangeKind,
    /// What changed.
    pub text: String,
    /// Test id (`task-<id>`, or `change-<seq>` without a task).
    pub test_id: String,
    /// Unseen styling, absent on phone rows.
    pub unseen: bool,
    /// Open the changed task.
    pub action: Action,
}

/// Empty-list or empty-detail message and its optional navigation action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyState {
    /// Message heading.
    pub title: &'static str,
    /// Complete explanatory paragraph.
    pub body: String,
    /// Optional recovery/navigation control.
    pub action: Option<Button>,
}

/// Inbox list content, including loading placeholders and stale/error fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxList {
    /// Loading suppresses both rows and the changes section.
    Loading {
        /// Five desktop list rows or two phone cards.
        skeleton_count: u8,
        /// Placeholder blocks per row/card.
        blocks_per_item: u8,
    },
    /// The projection is usable, including while stale or showing a refresh error.
    Ready {
        /// Desktop listbox accessibility name.
        label: &'static str,
        /// Refresh-error banner above the retained rows.
        error: Option<Notice>,
        /// Needs-you group heading.
        needs_title: &'static str,
        /// Count of rendered rows, including stamped answered questions.
        needs_count: usize,
        /// Needs-you rows/cards in order.
        items: Vec<NeedYouItem>,
        /// Empty-state message when there are no needs-you rows.
        empty: Option<EmptyState>,
        /// Desktop “Since your last visit” or phone “Since 18:40”.
        since_title: String,
        /// Desktop's time of the last visit in the group heading, when known.
        last_visit: Option<String>,
        /// Phone's count of changes since the last visit, independent of the preview length.
        changes_count: u64,
        /// Desktop or phone change rows.
        feed: Vec<FeedRow>,
        /// All-changes navigation control, already labelled for the form.
        all_changes: Button,
    },
}

/// A plan's small inbox preview, not the full plan-review document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPreview {
    /// Ordered needs-you and state chips.
    pub chips: Vec<Chip>,
    /// Plan id and revision.
    pub record: String,
    /// Full plan title.
    pub title: String,
    /// Plan goal, when the projection carries it.
    pub goal: Option<String>,
    /// Heading for the consequences paragraph.
    pub consequence_title: &'static str,
    /// Members, delivery destination and autopilot consequence.
    pub consequence: Rich,
    /// Open the full plan review.
    pub open: Button,
}

/// A served needs-you item that is neither a question, a prompt nor a plan: a review, a
/// stuck task or an escalation without a question record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPreview {
    /// Needs-you and state chips.
    pub chips: Vec<Chip>,
    /// Row id.
    pub record: String,
    /// Row title.
    pub title: String,
    /// Recorded facts, one per line.
    pub facts: Vec<String>,
    /// Open the item's own page, when it has one.
    pub open: Option<Button>,
}

/// Selected assistant prompt metadata in the fixture preview; the screen composes Permission
/// with `assistant::permission_card(env)` and Confirmation with `assistant::mode_pause_card()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantPreview {
    /// Selects the permission or mode-pause builder documented on this type.
    pub prompt: Selection,
    /// Needs-you header chip.
    pub chip: Chip,
    /// Source attribution above the selected prompt.
    pub attribution: &'static str,
    /// Full selected-pane title.
    pub title: &'static str,
    /// One-answer-wins explanation below the assistant-owned prompt card.
    pub note: &'static str,
    /// Open A-7's quick thread.
    pub open: Button,
}

/// The desktop right pane. Phone renders the list and pushes the selected item instead.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxPreview {
    /// Skeleton blocks while the projection or the question's detail loads.
    Loading {
        /// Header/title/body placeholder count.
        skeleton_count: u8,
    },
    /// No item is waiting on the owner.
    Empty(EmptyState),
    /// The selected question's detail did not load.
    Failed(Notice),
    /// Full inline S, B, A, R, Thread, History handover.
    Question(Sheet),
    /// A plan's goal and publication consequence.
    Plan(PlanPreview),
    /// The fixture's illustrative PM question preview.
    Pm {
        /// Needs-you chip.
        chip: Chip,
        /// PM question id.
        record: &'static str,
        /// Explicit sample attribution.
        attribution: &'static str,
        /// Question title.
        title: &'static str,
        /// Current repair-budget situation.
        body: &'static str,
    },
    /// A fixture assistant-domain prompt, with the surrounding inbox content resolved here.
    Assistant(AssistantPreview),
    /// A served assistant prompt: the assistant domain renders its card from the thread.
    Prompt {
        /// The assistant thread (`quick` or `A-n`).
        thread: Id,
        /// The prompt key.
        key: Id,
    },
    /// A served review, stuck task or escalation.
    Item(ItemPreview),
}

/// Complete Inbox destination content without application-shell banners or presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct Inbox {
    /// Phone page title and desktop accessibility title.
    pub title: &'static str,
    /// Whether the phone uses its large title treatment.
    pub large_title: bool,
    /// List or phone cards followed by the changes feed.
    pub list: InboxList,
    /// Desktop/terminal inline preview, absent on phone.
    pub preview: Option<InboxPreview>,
}

const PM_TITLE: &str = "T-852 repair budget: re-gate only, or one more round?";
const PLAN_TITLE: &str = "PM supervisor evolution: bounded reads";

fn fixture_id(key: Selection) -> &'static str {
    match key {
        Selection::Question => "inbox-question",
        Selection::Plan => "inbox-plan",
        Selection::Pm => "inbox-pm",
        Selection::Permission => "inbox-permission",
        Selection::Confirmation => "inbox-confirmation",
    }
}

/// Build all five fixture needs-you items, or none for Empty. The question is retained
/// with its answer stamp even though needs_you_count no longer includes it in the badge.
pub fn need_you(state: &State, env: Env) -> Vec<NeedYouItem> {
    if env.mode == DataMode::Empty {
        return Vec::new();
    }
    let status = state.question_state(env);
    let row = |key: Selection,
               lead: &str,
               id: &str,
               title: &str,
               meta: String,
               age: &str,
               question_state: Option<QuestionState>,
               sample| NeedYouItem {
        test_id: fixture_id(key).into(),
        lead: lead.into(),
        id: id.into(),
        heading: vec![span(lead), span(" · "), span(id).mono()],
        title: title.into(),
        meta,
        age: age.into(),
        state: question_state,
        hue: if question_state.is_some_and(|st| st != QuestionState::Open) {
            Hue::Neutral
        } else {
            Hue::You
        },
        selected: state.sel == key,
        sample,
        action: Action::Select(key),
    };
    let mut items = vec![
        (
            Selection::Question,
            row(
                Selection::Question,
                "Worker asks",
                Q.key,
                Q.short,
                match status {
                    QuestionState::Elsewhere => "answered on PM pane 22:14".into(),
                    QuestionState::Mine => "answered by you 22:12".into(),
                    _ => format!("T-868 blocked · {} · owner only", Q.age),
                },
                Q.age,
                Some(status),
                None,
            ),
        ),
        (
            Selection::Plan,
            row(
                Selection::Plan,
                "Plan awaits acceptance",
                "T-885 P2",
                PLAN_TITLE,
                "reviewed · approve P2 · 3 members".into(),
                "10h",
                None,
                None,
            ),
        ),
        (
            Selection::Pm,
            row(
                Selection::Pm,
                "PM asks",
                "PM/Q7",
                PM_TITLE,
                "12m · nothing blocked".into(),
                "12m",
                None,
                Some("sample"),
            ),
        ),
        (
            Selection::Permission,
            row(
                Selection::Permission,
                "Assistant A-7 asks",
                "write permission",
                "Write ~/miti-workspace/notes/corral-log.md (+6 lines)?",
                "Allow once · 1 hour · session · Deny".into(),
                "7m",
                None,
                None,
            ),
        ),
        (
            Selection::Confirmation,
            row(
                Selection::Confirmation,
                "Assistant A-7 waits",
                "corral mode pause",
                "Confirm on this device: pause autopilot for tonight",
                "confirm on this device · mode".into(),
                "10m",
                None,
                None,
            ),
        ),
    ];
    for (key, item) in &mut items {
        if phone(env) {
            item.selected = false;
            item.heading = vec![span(item.lead.clone()), span(format!(" · {}", item.age))];
            item.action = match key {
                Selection::Question => {
                    item.heading.push(span(" · blocking"));
                    item.meta = match status {
                        QuestionState::Elsewhere => "Answered on PM pane 22:14 · option 1".into(),
                        QuestionState::Mine => {
                            format!("Answered by you · option {}", state.choice + 1)
                        }
                        _ => "T-868 host-queue-ledger · 2 options · worker recommends 1".into(),
                    };
                    Action::OpenQuestion
                }
                Selection::Plan => {
                    item.title = format!("T-885 P2 · {PLAN_TITLE}");
                    item.meta = "Reviewer approved P2 · accepting publishes 3 members".into();
                    Action::PlanOpen("T-885")
                }
                Selection::Pm => {
                    item.heading.extend([span(" · "), span("sample").dim()]);
                    item.meta = "Nothing blocked · PM keeps other work moving".into();
                    Action::Toast("Sample question (illustrative)")
                }
                Selection::Permission | Selection::Confirmation => {
                    item.meta.push_str(" · answers in the quick thread");
                    Action::Open(Page::Thread("A-7"))
                }
            };
        } else if item.sample.is_some() {
            item.heading.extend([span(" · "), span("sample").dim()]);
        }
    }
    items.into_iter().map(|(_, item)| item).collect()
}

/// Build a bounded fixture changes feed from the shared board.
pub fn feed(limit: usize, env: Env) -> Vec<FeedRow> {
    board::changes()
        .iter()
        .take(limit)
        .map(|change| FeedRow {
            at: change.at.into(),
            id: change.id.into(),
            kind: change.kind,
            text: change.text.into(),
            test_id: format!("task-{}", change.id),
            unseen: !phone(env),
            action: Action::Open(Page::Task(change.id)),
        })
        .collect()
}

/// Build the fixture's desktop list or phone cards, their error banner, empty state, and
/// changes preview. Empty still shows the changes feed; Loading does not.
pub fn fixture_list(state: &State, env: Env) -> InboxList {
    if env.mode == DataMode::Loading {
        return InboxList::Loading {
            skeleton_count: if phone(env) { 2 } else { 5 },
            blocks_per_item: if phone(env) { 3 } else { 2 },
        };
    }
    let items = need_you(state, env);
    let error = (env.mode == DataMode::Error).then(|| {
        if phone(env) {
            Notice {
                stamp: None,
                hue: Hue::Fail,
                body: vec![
                    span("Couldn't refresh.").bold(),
                    span(" corrald answered with an error on "),
                    span("board.subscribe").mono(),
                    span(". Showing the last state from 22:08. Pull to retry."),
                ],
                retry: None,
            }
        } else {
            Notice {
                stamp: None,
                hue: Hue::Fail,
                body: vec![
                    span("Couldn't refresh the inbox.").bold(),
                    span(" corrald returned an error on "),
                    span("sub.open inbox").mono(),
                    span(". Showing the last state from 22:08; retrying every 10 s. "),
                ],
                retry: Some(button(
                    "Retry now",
                    Action::Toast("Retrying…"),
                    ButtonKind::Quiet,
                )),
            }
        }
    });
    let empty = items.is_empty().then(|| EmptyState {
        title: "Nothing needs you.",
        body: if phone(env) {
            "Corral is verifying 3 · workers producing 2 · 23 queued. You'll get a push when that changes."
        } else {
            "Corral is verifying 3, workers are producing 2, 23 are queued. You'll get a push if that changes."
        }
        .into(),
        action: Some(button(
            "Open Now",
            Action::Go(Destination::Now),
            ButtonKind::Ghost,
        )),
    });
    InboxList::Ready {
        label: "Inbox",
        error,
        needs_title: "Needs you",
        needs_count: items.len(),
        items,
        empty,
        since_title: if phone(env) {
            format!("Since {}", board::LAST_VISIT)
        } else {
            "Since your last visit".into()
        },
        last_visit: Some(board::LAST_VISIT.into()),
        changes_count: 12,
        feed: feed(if phone(env) { 6 } else { 8 }, env),
        all_changes: button(
            if phone(env) { "All" } else { "All 12 changes" },
            Action::Go(Destination::Changes),
            ButtonKind::Quiet,
        ),
    }
}

/// Build the fixture's selected desktop pane; assistant prompt cards remain owned by the
/// assistant model and are composed using the typed prompt identity on AssistantPreview.
pub fn fixture_preview(state: &State, env: Env) -> InboxPreview {
    if env.mode == DataMode::Loading {
        return InboxPreview::Loading { skeleton_count: 3 };
    }
    if env.mode == DataMode::Empty {
        return InboxPreview::Empty(EmptyState {
            title: "Inbox is clear",
            body: "Answered questions and accepted plans stay in Changes and Timeline. Nothing here is waiting on you.".into(),
            action: None,
        });
    }
    match state.sel {
        Selection::Question => InboxPreview::Question(fixture_desk(state, env, false)),
        Selection::Plan => InboxPreview::Plan(PlanPreview {
            chips: vec![
                Chip {
                    label: "needs you",
                    hue: Hue::You,
                },
                Chip {
                    label: "reviewed · approve P2",
                    hue: Hue::Ok,
                },
            ],
            record: "T-885 P2".into(),
            title: "PM supervisor evolution: bounded reads, warm-context episodes".into(),
            goal: Some("Deliver a bounded first wave of PM information and lifecycle visibility improvements, while reviewing a longer-term hybrid supervisor design without authorizing it wholesale.".into()),
            consequence_title: "What accepting does",
            consequence: vec![
                span("Publishes 3 member tasks to "),
                span("main").bold(),
                span(" delivery: "),
                span("pm-bounded-read-contract").mono(),
                span(" → "),
                span("pm-obligation-read-view").mono(),
                span(" → "),
                span("pm-read-costs-and-guidance").mono(),
                span(". Autopilot may then admit the first one."),
            ],
            open: button(
                "Open plan review",
                Action::PlanOpen("T-885"),
                ButtonKind::Primary,
            ),
        }),
        Selection::Pm => InboxPreview::Pm {
            chip: Chip {
                label: "needs you",
                hue: Hue::You,
            },
            record: "PM/Q7",
            attribution: "sample question",
            title: PM_TITLE,
            body: "T-852 has spent $43 (warning at $20) over 2 review rounds. Its gate went stale because the base moved, not because of a code defect.",
        },
        Selection::Permission | Selection::Confirmation => {
            InboxPreview::Assistant(AssistantPreview {
                prompt: state.sel,
                chip: Chip {
                    label: "needs you",
                    hue: Hue::You,
                },
                attribution: "from the quick thread · A-7 · you were not in the thread",
                title: if state.sel == Selection::Permission {
                    "Assistant wants to write a file"
                } else {
                    "Confirm corral mode pause"
                },
                note: "One answer wins: answering here also settles it in the quick thread on every device.",
                open: button(
                    "Open the quick thread",
                    Action::Open(Page::Thread("A-7")),
                    ButtonKind::Quiet,
                ),
            })
        }
    }
}
