//! The build tools' own cards, as `ui/build` draws them. `task`: `task · n slices, one
//! worktree`, one row per slice (state, id bold, title, `profile · model`, time) from the live
//! fleet while it knows the slice, else the settled result, else the arguments with `outcome
//! unknown`; under it one line per slice that came back. `build_finish`: `Build finished · d/n
//! slices · time`, one row per slice, the branch figures, outside edits, the record's gate
//! attempts and review rounds and the exit's progress on the latest card only (the host's Land
//! steps and gate log when it reports them; an older card says it was superseded). `board_plan`: one row with the plan's size, and whether the owner
//! still has to confirm it.

use corral_protocol::build::BuildRecord;
use corral_protocol::detail::{CardKind, DetailBody, DetailRef};
use corral_protocol::session_state::{ExitBrief, ExitChoice, ExitPhase};
use corral_protocol::subscription::SessionChild;
use serde_json::Value;

use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::session::land::{self, GateLog};
use crate::model::session::{StepState, Tab};

use super::Action;
use super::build_entries::{ExitEntry, FiledEntry, Told};
use super::replay::Cx;
use super::tool::{Call, CallState, ToolState, head, list_of, plural, str_of};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildCard {
    Task(TaskCard),
    Finish(FinishCard),
    Plan(PlanCard),
    /// The owner's exit choice and how it ended (`corral_build_exit`).
    Exit(ExitEntry),
    /// The tasks the owner filed on the board (`corral_build_board`).
    Filed(FiledEntry),
}

/// The `task` card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCard {
    /// `task · 2 slices, one worktree`.
    pub head: String,
    pub rows: Vec<SliceRow>,
    /// One line per slice that came back, in the order they ended.
    pub back: Vec<SliceBack>,
    /// The call failed before any slice started: its first line.
    pub error: Option<String>,
    /// Opens the session's Agents tab.
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceRow {
    pub state: ToolState,
    pub id: String,
    pub title: String,
    /// `default · gpt-6.1`, `review · deep`.
    pub profile: String,
    /// The time, `waiting`, `needs you`, `outcome unknown`, in `hue`.
    pub time: String,
    pub hue: Hue,
}

/// `✓ host-slots finished · 2 files · cargo test slots 9 passed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceBack {
    pub state: ToolState,
    pub id: String,
    pub words: String,
}

/// The `build_finish` card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishCard {
    /// `Build finished · 2/2 slices · 14m`.
    pub title: String,
    pub slices: Vec<SliceRow>,
    /// `14 files +412 −88 on build/auth, 3 commits`.
    pub figures: Option<String>,
    /// Warnings: outside edits.
    pub flags: Vec<String>,
    pub verification: Vec<GateRow>,
    pub reviews: Vec<ReviewRow>,
    /// The parent's own note.
    pub note: Option<String>,
    /// `you chose Land on main`, then the exit's progress and outcome; the latest card only.
    pub chose: Option<String>,
    pub exit: Option<ExitLine>,
    pub steps: Vec<ExitStep>,
    pub superseded: bool,
    pub action: Action,
    /// The words the host told the agent with an owner-opened finish card, while opened.
    pub told: Option<Told>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRow {
    pub attempt: String,
    pub tier: String,
    pub state: ToolState,
    pub word: &'static str,
    pub command: Option<String>,
    pub time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRow {
    /// `R1  ✗ changes · 1 blocker, 2 major · gpt-6.1`.
    pub round: String,
    pub state: ToolState,
    pub words: String,
    /// Unresolved findings: severity in its hue, then `path:line  text`.
    pub findings: Vec<(String, Hue, String)>,
    pub resolved: Option<String>,
}

/// How the exit stands: a glyph and sentence, the next step, facts muted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitLine {
    pub state: ToolState,
    pub words: String,
    pub step: Option<String>,
    pub facts: Option<String>,
}

/// One step of Land: `✓ Rebase onto main`, `⠹ Full gate`, `○ Land (fast-forward main)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStep {
    pub state: ToolState,
    pub words: String,
    /// The host's fact about the step, muted after the words: `3f2a9c1`, `G4 passed`.
    pub note: Option<String>,
    /// `3m 12s`.
    pub time: Option<String>,
    /// The gate's log, under the gate step.
    pub log: Option<GateLog>,
}

/// The `board_plan` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanCard {
    pub state: ToolState,
    pub title: String,
    /// `4 tasks, 3 dependency edges`.
    pub size: String,
    /// `◆ waiting for your confirmation` while the plan is the open card.
    pub waiting: bool,
    /// The result's first line once confirmed or declined, or the failure.
    pub said: Option<String>,
}

/// A build tool's call as its card. `partial` says only a clipped preview of the call is here
/// yet (its complete item is fetched on sight).
pub(super) fn card(call: &Call<'_>, call_id: &str, partial: bool, cx: Cx<'_>) -> BuildCard {
    let agents = Action::Tab {
        session: cx.id.clone(),
        tab: Tab::Agents,
    };
    match call.name {
        "task" => BuildCard::Task(task(call, partial, cx, agents)),
        "build_finish" => BuildCard::Finish(finish(
            call_id,
            call.arg("summary").map(str::to_owned),
            cx,
            agents,
        )),
        _ => BuildCard::Plan(plan(call, cx)),
    }
}

/// The session's live children, as its host last reported them.
fn fleet<'a>(cx: Cx<'a>) -> &'a [SessionChild] {
    cx.feed
        .live
        .get(cx.id.as_str())
        .and_then(|live| live.fleet.as_ref())
        .map_or(&[], |fleet| fleet.children.as_slice())
}

/// `3m 12s` between two epoch-millisecond times, or to now.
fn span(started: Option<u64>, ended: Option<u64>, feed: &Feed) -> Option<String> {
    let started = started?;
    let end = ended.unwrap_or(feed.now.saturating_mul(1000));
    (end >= started).then(|| clock_span(end - started))
}

fn clock_span(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// `review · deep` for a child that is not a slice, else the profile.
fn profile(kind: Option<&str>, profile: &str) -> String {
    match kind {
        Some(kind @ ("explore" | "review")) => format!("{kind} · {profile}"),
        _ => profile.to_owned(),
    }
}

/// A slice row in the state its record says.
fn slice(id: &str, title: &str, profile: String, state: &str, time: Option<String>) -> SliceRow {
    let (glyph, words, hue) = match state {
        "queued" => (ToolState::Waiting, "waiting".to_owned(), Hue::Neutral),
        "running" => (
            ToolState::Running,
            time.unwrap_or_else(|| "running".into()),
            Hue::Neutral,
        ),
        "needs" => (ToolState::NeedsYou, "needs you".to_owned(), Hue::You),
        "done" => (
            ToolState::Ok,
            time.unwrap_or_else(|| "done".into()),
            Hue::Ok,
        ),
        "failed" => (ToolState::Failed, "failed".to_owned(), Hue::Fail),
        "cancelled" => (ToolState::Failed, "cancelled".to_owned(), Hue::Fail),
        _ => (ToolState::Unknown, "outcome unknown".to_owned(), Hue::Held),
    };
    SliceRow {
        state: glyph,
        id: id.to_owned(),
        title: title.to_owned(),
        profile,
        time: words,
        hue,
    }
}

fn live_row(child: &SessionChild, feed: &Feed) -> SliceRow {
    let label = profile(
        Some(child.kind.as_str()),
        child.profile.as_deref().unwrap_or("default"),
    );
    let model = child.model.rsplit('/').next().unwrap_or(&child.model);
    slice(
        &child.id,
        &child.title,
        if model.is_empty() {
            label
        } else {
            format!("{label} · {model}")
        },
        &child.state,
        span(child.started_at, child.ended_at, feed),
    )
}

/// `approved · no findings`, `changes requested · 1 blocker, 2 major`.
fn verdict_words(verdict: &Value) -> String {
    let findings = list_of(verdict, "findings");
    let counts: Vec<String> = ["blocker", "major", "minor"]
        .into_iter()
        .filter_map(|severity| {
            let n = findings
                .iter()
                .filter(|finding| str_of(finding, "severity") == Some(severity))
                .count();
            (n > 0).then(|| format!("{n} {severity}"))
        })
        .collect();
    let lead = if str_of(verdict, "verdict") == Some("approve") {
        "approved"
    } else {
        "changes requested"
    };
    if counts.is_empty() {
        format!("{lead} · no findings")
    } else {
        format!("{lead} · {}", counts.join(", "))
    }
}

/// What came back from one settled slice.
fn back(child: &Value) -> Option<SliceBack> {
    let id = str_of(child, "id")?.to_owned();
    match str_of(child, "state")? {
        "done" => {
            let what = match child.get("verdict") {
                Some(verdict) if verdict.is_object() => verdict_words(verdict),
                _ => {
                    let edits = list_of(child, "edits");
                    let mut inside: Vec<&str> = Vec::new();
                    let mut outside: Vec<&str> = Vec::new();
                    for edit in edits {
                        let Some(path) = str_of(edit, "path") else {
                            continue;
                        };
                        let list = if edit.get("outside").and_then(Value::as_bool) == Some(true) {
                            &mut outside
                        } else {
                            &mut inside
                        };
                        if !list.contains(&path) {
                            list.push(path);
                        }
                    }
                    inside.retain(|path| !outside.contains(path));
                    let mut words = plural(inside.len() as u64, "file", "files");
                    if !outside.is_empty() {
                        words.push_str(&format!(" + {} outside its set", outside.len()));
                    }
                    words
                }
            };
            let report = head(str_of(child, "report").unwrap_or_default())
                .trim_start_matches('#')
                .trim()
                .to_owned();
            let words = if report.is_empty() {
                format!("finished · {what}")
            } else {
                format!("finished · {what} · {report}")
            };
            Some(SliceBack {
                state: ToolState::Ok,
                id,
                words,
            })
        }
        "failed" => {
            let error = head(str_of(child, "error").unwrap_or_default());
            Some(SliceBack {
                state: ToolState::Failed,
                id,
                words: if error.is_empty() {
                    "failed".to_owned()
                } else {
                    format!("failed · {error}")
                },
            })
        }
        "cancelled" => Some(SliceBack {
            state: ToolState::Failed,
            id,
            words: "cancelled · its edits so far stay in the worktree".to_owned(),
        }),
        _ => None,
    }
}

fn task(call: &Call<'_>, partial: bool, cx: Cx<'_>, action: Action) -> TaskCard {
    let live = fleet(cx);
    let settled = list_of(&call.details, "children");
    // The agents the call passed (`agents` in the host's `task` schema), in its order; the
    // settled result names them when the arguments are not here.
    let specs = list_of(&call.args, "agents");
    let ids: Vec<&str> = if specs.is_empty() {
        settled
            .iter()
            .filter_map(|child| str_of(child, "id"))
            .collect()
    } else {
        specs.iter().filter_map(|spec| str_of(spec, "id")).collect()
    };
    let rows: Vec<SliceRow> = ids
        .iter()
        .map(|id| {
            if let Some(child) = live.iter().find(|child| child.id == *id) {
                return live_row(child, cx.feed);
            }
            let record = settled
                .iter()
                .chain(specs)
                .find(|item| str_of(item, "id") == Some(*id))
                .unwrap_or(&Value::Null);
            let label = profile(
                str_of(record, "kind"),
                str_of(record, "profile").unwrap_or("default"),
            );
            let model = str_of(record, "model")
                .map(|model| model.rsplit('/').next().unwrap_or(model).to_owned());
            slice(
                id,
                str_of(record, "title").unwrap_or_default(),
                match model {
                    Some(model) => format!("{label} · {model}"),
                    None => label,
                },
                str_of(record, "state").unwrap_or("unknown"),
                span(
                    record.get("startedAt").and_then(Value::as_u64),
                    record.get("endedAt").and_then(Value::as_u64),
                    cx.feed,
                ),
            )
        })
        .collect();
    let mut ended: Vec<(&Value, u64)> = settled
        .iter()
        .map(|child| {
            (
                child,
                child.get("endedAt").and_then(Value::as_u64).unwrap_or(0),
            )
        })
        .collect();
    ended.sort_by_key(|(_, at)| *at);
    let count = rows.len() as u64;
    TaskCard {
        head: if count == 0 && partial {
            "task · reading the agents it started…".to_owned()
        } else {
            format!("task · {}, one worktree", plural(count, "slice", "slices"))
        },
        rows,
        back: ended
            .into_iter()
            .filter_map(|(child, _)| back(child))
            .collect(),
        error: (call.state == CallState::Failed && settled.is_empty()).then(|| {
            let first = head(call.text);
            if first.is_empty() {
                "The tool failed.".to_owned()
            } else {
                first.to_owned()
            }
        }),
        action,
    }
}

/// The exit's words for its phase; the host's detail wins over the default words.
fn exit_line(exit: &ExitBrief, worktree: Option<&str>, branch: Option<&str>) -> Option<ExitLine> {
    let detail = exit.detail.clone();
    let running = |words: &str| ExitLine {
        state: ToolState::Running,
        words: detail.clone().unwrap_or_else(|| words.to_owned()),
        step: None,
        facts: None,
    };
    Some(match exit.phase {
        ExitPhase::Rebasing => running("Rebasing onto the base branch."),
        ExitPhase::Verifying => running("Running the full gate on the rebased head."),
        ExitPhase::Repairing => {
            let round = exit
                .repair
                .map(|repair| format!(" · Repair {} of {}", repair.round, repair.of))
                .unwrap_or_default();
            ExitLine {
                state: ToolState::Unknown,
                words: format!(
                    "{}{round}",
                    detail
                        .clone()
                        .unwrap_or_else(|| "The full gate failed".to_owned())
                ),
                step: Some("The session is fixing it; Land resumes when it finishes.".to_owned()),
                facts: None,
            }
        }
        ExitPhase::Landing => running("Fast-forwarding the base branch."),
        ExitPhase::Kept => ExitLine {
            state: ToolState::Ok,
            words: detail.unwrap_or_else(|| match (branch, worktree) {
                (Some(branch), Some(worktree)) => format!("Kept on {branch} in {worktree}"),
                _ => "Kept on the branch.".to_owned(),
            }),
            step: None,
            facts: None,
        },
        ExitPhase::Landed => ExitLine {
            state: ToolState::Ok,
            words: match &exit.landed {
                Some(sha) => format!(
                    "Landed {}{}",
                    sha.chars().take(7).collect::<String>(),
                    exit.base_branch
                        .as_deref()
                        .map(|base| format!(" on {base}"))
                        .unwrap_or_default()
                ),
                None => detail.unwrap_or_else(|| "Landed.".to_owned()),
            },
            step: None,
            facts: None,
        },
        ExitPhase::Failed => ExitLine {
            state: ToolState::Failed,
            words: format!(
                "{} Nothing was landed; the branch and worktree are unchanged.",
                detail.unwrap_or_else(|| "The exit failed.".to_owned())
            ),
            step: Some("Inspect the worktree and choose Land again with Finish.".to_owned()),
            facts: None,
        },
        _ => return None,
    })
}

/// Land's steps: the host's own when it reports them, else those of the phase it is in.
fn steps(exit: &ExitBrief, now: u64) -> Vec<ExitStep> {
    let base = exit.base_branch.as_deref().unwrap_or("the base branch");
    if let Some(land) = &exit.land {
        return land::rows(land, base, now)
            .into_iter()
            .map(|row| ExitStep {
                state: match row.state {
                    StepState::Done => ToolState::Ok,
                    StepState::Running => ToolState::Running,
                    StepState::Failed => ToolState::Failed,
                    StepState::Waiting => ToolState::Waiting,
                },
                words: row.label,
                note: row.note,
                time: row.time,
                log: row.log,
            })
            .collect();
    }
    if exit.choice != Some(ExitChoice::Land) {
        return Vec::new();
    }
    let reached = match exit.phase {
        ExitPhase::Rebasing => 0,
        ExitPhase::Verifying | ExitPhase::Repairing => 1,
        ExitPhase::Landing => 2,
        ExitPhase::Landed => 3,
        _ => return Vec::new(),
    };
    [
        format!("Rebase onto {base}"),
        "Full gate".to_owned(),
        format!("Land (fast-forward {base})"),
    ]
    .into_iter()
    .enumerate()
    .map(|(at, words)| ExitStep {
        state: match at.cmp(&reached) {
            std::cmp::Ordering::Less => ToolState::Ok,
            std::cmp::Ordering::Equal if exit.phase == ExitPhase::Repairing => ToolState::Failed,
            std::cmp::Ordering::Equal => ToolState::Running,
            std::cmp::Ordering::Greater => ToolState::Waiting,
        },
        words,
        note: None,
        time: None,
        log: None,
    })
    .collect()
}

fn gate_rows(record: &BuildRecord) -> Vec<GateRow> {
    record
        .verification
        .iter()
        .map(|gate| {
            let (state, word) = match gate.result.as_str() {
                "pass" => (ToolState::Ok, "pass"),
                "fail" => (ToolState::Failed, "fail"),
                _ => (ToolState::Waiting, "skipped"),
            };
            let time = match (
                gate.started.parse::<jiff::Timestamp>(),
                gate.ended.parse::<jiff::Timestamp>(),
            ) {
                (Ok(start), Ok(end)) => {
                    u64::try_from(end.as_millisecond() - start.as_millisecond())
                        .ok()
                        .map(clock_span)
                }
                _ => None,
            };
            GateRow {
                attempt: gate.attempt.clone(),
                tier: gate.tier.clone(),
                state,
                word,
                command: gate
                    .failed_command
                    .clone()
                    .filter(|_| state == ToolState::Failed),
                time,
            }
        })
        .collect()
}

fn review_rows(record: &BuildRecord) -> Vec<ReviewRow> {
    record
        .reviews
        .iter()
        .map(|review| {
            let counts: Vec<String> = ["blocker", "major", "minor"]
                .into_iter()
                .filter_map(|severity| {
                    let n = review
                        .findings
                        .iter()
                        .filter(|finding| finding.severity == severity)
                        .count();
                    (n > 0).then(|| format!("{n} {severity}"))
                })
                .collect();
            let approve = review.verdict == "approve";
            let model = review.model.rsplit('/').next().unwrap_or(&review.model);
            let open: Vec<_> = review
                .findings
                .iter()
                .filter(|finding| {
                    finding
                        .resolution
                        .as_ref()
                        .is_none_or(|resolution| resolution.state == "open")
                })
                .collect();
            let resolved = review.findings.len() - open.len();
            ReviewRow {
                round: format!("R{}", review.round),
                state: if approve {
                    ToolState::Ok
                } else {
                    ToolState::Failed
                },
                words: format!(
                    "{} · {} · {model}",
                    if approve { "approve" } else { "changes" },
                    if counts.is_empty() {
                        "no findings".to_owned()
                    } else {
                        counts.join(", ")
                    }
                ),
                findings: open
                    .iter()
                    .map(|finding| {
                        let hue = match finding.severity.as_str() {
                            "blocker" => Hue::Fail,
                            "major" => Hue::Held,
                            _ => Hue::Neutral,
                        };
                        let place = match (&finding.path, finding.line) {
                            (Some(path), Some(line)) => format!("{path}:{line}  "),
                            (Some(path), None) => format!("{path}  "),
                            _ => String::new(),
                        };
                        (
                            finding.severity.clone(),
                            hue,
                            format!("{place}{}", finding.text),
                        )
                    })
                    .collect(),
                resolved: (resolved > 0).then(|| format!("{resolved} resolved")),
            }
        })
        .collect()
}

/// The result card of exit card `id` (a `build_finish` call, or the finish card the owner
/// opened): it follows the exit only while it is the page's latest exit card.
pub(super) fn finish(id: &str, note: Option<String>, cx: Cx<'_>, action: Action) -> FinishCard {
    let slices: Vec<SliceRow> = fleet(cx)
        .iter()
        .filter(|child| child.kind == "slice" || child.kind.is_empty())
        .map(|child| live_row(child, cx.feed))
        .collect();
    let done = slices
        .iter()
        .filter(|row| row.state == ToolState::Ok)
        .count();
    let started = fleet(cx).iter().filter_map(|child| child.started_at).min();
    let live_child = fleet(cx).iter().any(|child| child.ended_at.is_none());
    let ended = if live_child {
        None
    } else {
        fleet(cx).iter().filter_map(|child| child.ended_at).max()
    };
    let time = span(started, ended, cx.feed);
    let title = if slices.is_empty() {
        "Build finished".to_owned()
    } else {
        let mut title = format!("Build finished · {done}/{} slices", slices.len());
        if let Some(time) = time {
            title.push_str(&format!(" · {time}"));
        }
        title
    };
    let row = cx.feed.assistants.get(cx.id.as_str());
    let build = row.and_then(|row| row.build.as_ref());
    let figures = build.and_then(|build| {
        let diff = build.diff.as_ref()?;
        let branch = build
            .branch
            .as_deref()
            .map(|branch| format!(" on {branch}"))
            .unwrap_or_default();
        Some(format!(
            "{} +{} \u{2212}{}{branch}, {}",
            plural(diff.files, "file", "files"),
            diff.added,
            diff.removed,
            plural(diff.commits, "commit", "commits")
        ))
    });
    let flags = fleet(cx)
        .iter()
        .filter(|child| child.outside > 0)
        .map(|child| {
            format!(
                "! {} edited {} outside its files",
                child.id,
                plural(u64::from(child.outside), "file", "files")
            )
        })
        .collect();
    let latest = cx.journal.finish.as_deref() == Some(id);
    let record = latest
        .then(|| {
            cx.feed.detail(&DetailRef::BuildRecord {
                id: cx.id.to_string(),
            })
        })
        .flatten()
        .and_then(|cache| match cache.body()? {
            DetailBody::BuildRecord(record) => Some(record),
            _ => None,
        });
    let exit = latest
        .then(|| {
            cx.feed
                .live
                .get(cx.id.as_str())
                .and_then(|live| live.state.as_ref())
                .and_then(|state| state.exit.as_ref())
                .or_else(|| build.and_then(|build| build.exit.as_ref()))
        })
        .flatten();
    let chose = exit.and_then(|exit| exit.choice).and_then(|choice| {
        let label = match choice {
            ExitChoice::Keep => "Keep on the branch",
            ExitChoice::Land => "Land on the base branch",
            ExitChoice::Iterate => "Keep iterating",
            ExitChoice::Other => return None,
        };
        Some(format!("you chose {label}"))
    });
    let worktree = build.map(|build| build.worktree.as_str());
    let branch = build.and_then(|build| build.branch.as_deref());
    FinishCard {
        title,
        slices,
        figures,
        flags,
        verification: record.map(gate_rows).unwrap_or_default(),
        reviews: record.map(review_rows).unwrap_or_default(),
        note,
        chose,
        exit: exit.and_then(|exit| exit_line(exit, worktree, branch)),
        steps: exit
            .map(|exit| steps(exit, cx.feed.now))
            .unwrap_or_default(),
        superseded: !latest,
        action,
        told: None,
    }
}

fn plan(call: &Call<'_>, cx: Cx<'_>) -> PlanCard {
    let tasks = list_of(&call.args, "tasks");
    let edges: usize = tasks
        .iter()
        .map(|task| list_of(task, "depends").len())
        .sum();
    let title = call.arg("title").unwrap_or_default().to_owned();
    let waiting = call.state == CallState::Done
        && cx
            .feed
            .live
            .get(cx.id.as_str())
            .and_then(|live| live.state.as_ref())
            .is_some_and(|state| {
                state
                    .cards
                    .iter()
                    .any(|card| card.kind == CardKind::BoardPlan)
            });
    let said = match call.state {
        CallState::Done | CallState::Failed if !waiting => {
            let first = head(call.text);
            (!first.is_empty()).then(|| first.to_owned())
        }
        _ => None,
    };
    PlanCard {
        state: call.base(),
        title,
        size: format!(
            "{}, {}",
            plural(tasks.len() as u64, "task", "tasks"),
            plural(edges as u64, "dependency edge", "dependency edges")
        ),
        waiting,
        said,
    }
}
