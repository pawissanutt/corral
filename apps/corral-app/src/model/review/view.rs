//! Typed content for Review renderers. No widgets, dimensions or styling APIs live here.

use crate::model::board::Mark;
use crate::model::effect::{Conn, Env};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::{Rich, span};

use super::data::{
    CANDIDATES, Candidate, Comment, FILES, File, GATE_LOG, GATE_LOG_NOTE, HunkSource, LogLine,
    REPORT, Report, SCREENSHOT, Screenshot, VERDICTS,
};
use super::diff::{SplitRow, UnifiedRow, hunk_header, split, unified};
use super::{APPROVAL_REFUSED, Action, Layout, State, Tab};

/// A neutral or state-coloured chip whose copy is already resolved for the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    /// Chip text, without extra punctuation added by a renderer.
    pub label: &'static str,
    /// Semantic state colour.
    pub hue: Hue,
}

/// Button emphasis from the source, not a platform-specific layout instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Main decision.
    Primary,
    /// Secondary outlined decision or collapsed-file action.
    Ghost,
    /// Quiet inline action.
    Quiet,
}

/// A labelled intent; even a refused approval remains clickable so it can explain itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// Visible label.
    pub label: String,
    /// Reducer intent.
    pub action: Action,
    /// Whether the source affordance can be activated.
    pub enabled: bool,
    /// Source emphasis.
    pub style: ButtonStyle,
    /// Explanation produced on activation instead of executing an unavailable command.
    pub refusal: Option<&'static str>,
}

fn button(label: impl Into<String>, action: Action, style: ButtonStyle) -> Button {
    Button {
        label: label.into(),
        action,
        enabled: true,
        style,
        refusal: None,
    }
}

/// One candidate-list row with its static replay selection and toast-only action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRow {
    /// Candidate identity, state and actor mark colour.
    pub candidate: Candidate,
    /// Formatted task id and candidate identity.
    pub identity: Rich,
    /// T-868 remains selected even when other source rows are clicked.
    pub selected: bool,
    /// Intent for the whole row.
    pub action: Action,
}

/// Candidate group heading and its rows, absent from the phone page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateGroup {
    /// `Your verdict` or `Verifying`.
    pub label: &'static str,
    /// Group count displayed alongside the heading.
    pub count: u32,
    /// Heading's semantic state colour.
    pub hue: Hue,
    /// Rows in source order.
    pub rows: Vec<CandidateRow>,
}

/// The selected candidate's title, metadata and replay marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Selected task id.
    pub task: &'static str,
    /// Round shown with the candidate.
    pub round: &'static str,
    /// SHA whose approval is requested.
    pub sha: &'static str,
    /// Candidate title.
    pub title: &'static str,
    /// Form-specific metadata, including desktop base and totals.
    pub metadata: Rich,
    /// The phone marks this candidate as a replay here; desktop uses the list footnote.
    pub chips: Vec<Chip>,
}

/// A verdict's complete presentation, including stale mark, strike and older-candidate tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictView {
    /// Display mark; stale overrides the original fail mark.
    pub mark: Mark,
    /// Who supplied the verdict, in bold.
    pub who: &'static str,
    /// Candidate SHA with stale hue already applied.
    pub sha: Rich,
    /// Clock time, or empty for the owner.
    pub time: &'static str,
    /// Phone attribution line: bold actor, monospace SHA and optional clock time.
    pub line: Rich,
    /// Result text with strike/dim/owner hue resolved.
    pub text: Rich,
    /// Older-candidate tag, including its form-specific wording.
    pub tag: Option<Chip>,
}

/// A tab button in the desktop tab list or phone segmented list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabView {
    /// Tab identity.
    pub tab: Tab,
    /// Form-specific label.
    pub label: &'static str,
    /// Whether this tab is selected.
    pub selected: bool,
    /// Intent on activation.
    pub action: Action,
}

/// A layout preference button, shown only alongside the desktop Diff tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChoice {
    /// Preference this button writes.
    pub layout: Layout,
    /// `Side by side` or `Unified`.
    pub label: &'static str,
    /// Whether it matches the stored preference, not the width-constrained output.
    pub selected: bool,
    /// Reducer intent.
    pub action: Action,
}

/// An inline thread followed by the source's noninteractive owner reply box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentView {
    /// Reviewer, review round and candidate attribution.
    pub attribution: Rich,
    /// Plain comment body.
    pub text: &'static str,
    /// Static reply-box prompt, not a persisted comment draft.
    pub placeholder: &'static str,
    /// Keycap shown by the source, not a registered screen shortcut.
    pub keycap: &'static str,
    /// Inert source affordance; activation does not invent a comment editor.
    pub action: Action,
    /// False because the source provides neither input nor an action handler.
    pub editable: bool,
}

fn comment_view(comment: Comment) -> CommentView {
    CommentView {
        attribution: vec![
            span(comment.who).bold(),
            span(" · "),
            span(comment.at),
            span(" · on "),
            span(comment.sha),
        ],
        text: comment.text,
        placeholder: "Reply or add a line comment as owner…",
        keycap: "c",
        action: Action::CommentBox,
        editable: false,
    }
}

/// A file body. Row comments specify where to insert the file's resolved `discussion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffContent {
    /// One-stream gutters and code; insert discussion after each row with `comment: Some`.
    Unified(Vec<UnifiedRow>),
    /// Paired old/new cells; insert discussion after each row with `comment: Some`.
    Split(Vec<SplitRow>),
    /// Changed-line summary; clicking only shows the source expansion toast.
    Collapsed(Button),
}

/// A file heading with its counts, illustrative tag, hunk action and resolved diff body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    /// Full source path, also useful as a stable rendering key.
    pub path: &'static str,
    /// Monospace path followed by coloured added and deleted counts.
    pub heading: Rich,
    /// `lines illustrative` only for the fabricated cross-lane code lines.
    pub illustrative: Option<&'static str>,
    /// Ask action appears only on the file with the inline comment.
    pub ask: Option<Button>,
    /// Hunk header; absent on collapsed files.
    pub hunk_header: Option<&'static str>,
    /// Fully attributed discussion used wherever a row carries a comment attachment.
    pub discussion: Option<CommentView>,
    /// Resolved split, unified or collapsed body.
    pub content: DiffContent,
}

/// The Diff tab with the effective, form/width-constrained presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    /// Actual row presentation, not merely the stored preference.
    pub layout: Layout,
    /// Phone's explanatory sentence; absent from desktop and terminal.
    pub notice: Option<&'static str>,
    /// All five files in source order.
    pub files: Vec<FileView>,
}

/// Rendered report data with its styled provenance sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportView {
    /// Headings, paragraphs and marked acceptance list.
    pub document: &'static Report,
    /// Source label, monospace artifact and condensed-text note.
    pub source: Rich,
}

/// Terminal screenshot illustration and its real artifact caption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotView {
    /// Original text and description, including illustrative provenance.
    pub evidence: &'static Screenshot,
    /// Preformatted terminal text with the source's bold and success runs.
    pub terminal: Rich,
    /// Artifact path and the real-title/illustrative-content explanation.
    pub caption: Rich,
    /// Source shows a cursor after the final prompt.
    pub cursor: bool,
}

/// Exactly one evidence body selected by state and form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// File diffs and inline comments.
    Diff(DiffView),
    /// Rendered implementation report.
    Report(ReportView),
    /// Terminal capture.
    Screenshot(ScreenshotView),
    /// Desktop or terminal gate-log lines and their illustrative label.
    Log {
        /// Preformatted log lines with success hue resolved.
        lines: &'static [LogLine],
        /// Label under the log.
        note: &'static str,
    },
}

/// Candidate-bound decision controls; both forms route approval through confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecideBar {
    /// Buttons in source order: approve/changes on desktop, changes/approve on phone.
    pub buttons: Vec<Button>,
    /// Desktop's candidate-bound verdict explanation; absent on phone.
    pub explanation: Option<Rich>,
    /// Desktop's owner/device attribution; absent on phone as in the source.
    pub attribution: Option<Rich>,
}

/// Complete content of Review; the shell supplies shared connection banners and navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageView {
    /// Pushed-page title on phone.
    pub title: &'static str,
    /// Phone back-control label; absent on desktop and terminal.
    pub back_label: Option<&'static str>,
    /// Whether to use the phone content variant.
    pub phone: bool,
    /// Reading the last known state while offline or down.
    pub stale: bool,
    /// Connection state for the shared shell's banner.
    pub conn: Conn,
    /// Desktop candidate groups; empty on phone.
    pub candidates: Vec<CandidateGroup>,
    /// Desktop list footnote identifying the T-868 replay.
    pub candidate_note: Option<&'static str>,
    /// Selected candidate header and replay chips.
    pub header: Header,
    /// Form-specific verdict section heading.
    pub verdict_heading: &'static str,
    /// Phone's right-hand verdict heading label.
    pub verdict_binding: Option<&'static str>,
    /// Verdict rows, including the struck old candidate.
    pub verdicts: Vec<VerdictView>,
    /// Form-specific evidence-tab list.
    pub tabs: Vec<TabView>,
    /// Desktop Diff preference controls; empty on phone, terminal or other tabs.
    pub layouts: Vec<LayoutChoice>,
    /// Selected evidence, with phone Gate-log fallback matching the source.
    pub body: Body,
    /// Pinned decision controls.
    pub decide: DecideBar,
}

/// Resolves the Diff idiom rule: only desktop widths ≥ 900 pt may show split rows.
pub fn effective_layout(state: &State, env: Env, width: f32) -> Layout {
    if env.form == Form::Desktop && width >= 900.0 {
        state.layout
    } else {
        Layout::Unified
    }
}

/// Builds all file headings and bodies using an already resolved effective layout.
pub fn files(layout: Layout) -> Vec<FileView> {
    FILES.iter().map(|file| file_view(file, layout)).collect()
}

fn file_view(file: &File, layout: Layout) -> FileView {
    let content = if file.hunk == HunkSource::Collapsed {
        DiffContent::Collapsed(button(
            format!(
                "{} changed lines collapsed · expand",
                file.added + file.deleted
            ),
            Action::ExpandFile(file.path),
            ButtonStyle::Ghost,
        ))
    } else {
        match layout {
            Layout::Split => DiffContent::Split(split(file)),
            Layout::Unified => DiffContent::Unified(unified(file)),
        }
    };
    FileView {
        path: file.path,
        heading: vec![
            span(file.path).mono(),
            span(format!(" +{}", file.added)).hue(Hue::Ok),
            span(format!(" −{}", file.deleted)).hue(Hue::Fail),
        ],
        illustrative: file.illustrative.then_some("lines illustrative"),
        ask: file
            .comment
            .map(|_| button("Ask about this hunk", Action::AskHunk, ButtonStyle::Quiet)),
        hunk_header: hunk_header(file),
        discussion: file.comment.map(comment_view),
        content,
    }
}

/// Resolves stale strike, stale mark, owner hue and form-specific older-candidate chips.
pub fn verdicts(phone: bool) -> Vec<VerdictView> {
    VERDICTS
        .iter()
        .map(|verdict| VerdictView {
            mark: if verdict.stale {
                Mark::Stale
            } else {
                verdict.mark
            },
            who: verdict.who,
            sha: vec![span(verdict.sha).mono().hue(if verdict.stale && !phone {
                Hue::Held
            } else {
                Hue::Neutral
            })],
            time: verdict.time,
            line: {
                let mut line = vec![
                    span(verdict.who).bold(),
                    span(" · "),
                    span(verdict.sha).mono(),
                ];
                if !verdict.time.is_empty() {
                    line.push(span(" "));
                    line.push(span(verdict.time));
                }
                line
            },
            text: vec![if verdict.stale {
                span(verdict.text).strike().dim()
            } else {
                span(verdict.text).hue(if verdict.who == "you" && !phone {
                    Hue::You
                } else {
                    Hue::Neutral
                })
            }],
            tag: verdict.stale.then_some(Chip {
                label: if phone {
                    "older candidate"
                } else {
                    "stale · older candidate"
                },
                hue: Hue::Held,
            }),
        })
        .collect()
}

fn report() -> ReportView {
    ReportView {
        document: &REPORT,
        source: vec![
            span("Source: ").dim(),
            span(REPORT.source).mono().dim(),
            span(REPORT.source_note).dim(),
        ],
    }
}

fn screenshot() -> ScreenshotView {
    ScreenshotView {
        evidence: &SCREENSHOT,
        terminal: vec![
            span(SCREENSHOT.heading).bold(),
            span("\n\n› draft kept: \"tighten the withdraw race test\"\n\n"),
            span("Esc").bold(),
            span("  aborted turn · restored 1 pane item\n     "),
            span("external identities retained: 2").hue(Hue::Ok),
            span("\n     queued  1 steering · 1 follow-up\n\n› tighten the withdraw race test"),
        ],
        caption: vec![
            span(SCREENSHOT.source).dim(),
            span(" · ").dim(),
            span(SCREENSHOT.description).dim(),
        ],
        cursor: true,
    }
}

fn candidate_groups() -> Vec<CandidateGroup> {
    let row = |candidate: &Candidate| CandidateRow {
        candidate: *candidate,
        identity: vec![
            span(format!("{} · {}", candidate.task, candidate.sha))
                .mono()
                .dim(),
        ],
        selected: candidate.task == "T-868",
        action: Action::Select(candidate.task),
    };
    vec![
        CandidateGroup {
            label: "Your verdict",
            count: 1,
            hue: Hue::You,
            rows: CANDIDATES[..1].iter().map(row).collect(),
        },
        CandidateGroup {
            label: "Verifying",
            count: 3,
            hue: Hue::Corral,
            rows: CANDIDATES[1..].iter().map(row).collect(),
        },
    ]
}

fn decide(env: Env) -> DecideBar {
    let phone = env.form == Form::Phone;
    let mut approve = button("Approve 85b45e1e", Action::Approve, ButtonStyle::Primary);
    approve.refusal = env.conn.refuses().then_some(APPROVAL_REFUSED);
    let changes = button(
        if phone { "Changes" } else { "Request changes" },
        Action::RequestChanges,
        ButtonStyle::Ghost,
    );
    DecideBar {
        buttons: if phone {
            vec![changes, approve]
        } else {
            vec![approve, changes]
        },
        explanation: (!phone).then(|| {
            vec![
                span("Verdicts apply to "),
                span("85b45e1e").mono(),
                span(" only; a new candidate makes them stale. Approving needs a fresh confirm."),
            ]
        }),
        attribution: (!phone)
            .then(|| vec![span(format!("actor=owner · {}", env.via())).mono().dim()]),
    }
}

/// Builds Review with a wide desktop assumption; phone and terminal always remain unified.
/// Width-aware desktop renderers should call `page_for_width` instead.
pub fn page(state: &State, env: Env) -> PageView {
    page_for_width(state, env, f32::INFINITY)
}

/// Builds the phone variant even if the caller's environment has another form.
pub fn phone(state: &State, env: Env) -> PageView {
    page_for_width(
        state,
        Env {
            form: Form::Phone,
            ..env
        },
        0.0,
    )
}

/// Builds all page content at the available width, applying the 900 pt split threshold.
/// The source does not branch Review on data mode, so loading/empty/error keep this replay.
pub fn page_for_width(state: &State, env: Env, width: f32) -> PageView {
    let phone = env.form == Form::Phone;
    let layout = effective_layout(state, env, width);
    let tab_specs: &[(Tab, &str)] = if phone {
        &[
            (Tab::Diff, "Diff"),
            (Tab::Report, "Report"),
            (Tab::Shot, "Screen"),
        ]
    } else {
        &[
            (Tab::Diff, "Diff · 5 files"),
            (Tab::Report, "Report"),
            (Tab::Shot, "Screenshot"),
            (Tab::Log, "Gate log"),
        ]
    };
    let body = match state.tab {
        Tab::Diff => Body::Diff(DiffView {
            layout,
            notice: phone.then_some("Unified on phones; side by side needs ≥ 900 pt."),
            files: files(layout),
        }),
        Tab::Report => Body::Report(report()),
        Tab::Shot => Body::Screenshot(screenshot()),
        Tab::Log if phone => Body::Screenshot(screenshot()),
        Tab::Log => Body::Log {
            lines: GATE_LOG,
            note: GATE_LOG_NOTE,
        },
    };
    PageView {
        title: "Review",
        back_label: phone.then_some("Back"),
        phone,
        stale: env.conn.refuses(),
        conn: env.conn,
        candidates: if phone {
            Vec::new()
        } else {
            candidate_groups()
        },
        candidate_note: (!phone).then_some(
            "T-868 here is a replay of its 2026-11-08 review; the rest follow the snapshot.",
        ),
        header: Header {
            task: "T-868",
            round: "R1",
            sha: "85b45e1e",
            title: "host-queue-ledger",
            metadata: if phone {
                vec![span("T-868 · R1 · 85b45e1e").mono().dim()]
            } else {
                vec![
                    span("T-868 · R1 · candidate 85b45e1e").mono(),
                    span(" base main · 5 files · +334 −28"),
                ]
            },
            chips: if phone {
                vec![Chip {
                    label: "replay",
                    hue: Hue::Neutral,
                }]
            } else {
                Vec::new()
            },
        },
        verdict_heading: if phone {
            "Verdicts"
        } else {
            "Verdicts · bound to candidate SHA"
        },
        verdict_binding: phone.then_some("bound to SHA"),
        verdicts: verdicts(phone),
        tabs: tab_specs
            .iter()
            .map(|&(tab, label)| TabView {
                tab,
                label,
                selected: state.tab == tab,
                action: Action::Tab(tab),
            })
            .collect(),
        layouts: if state.tab == Tab::Diff && env.form == Form::Desktop {
            [
                (Layout::Split, "Side by side"),
                (Layout::Unified, "Unified"),
            ]
            .into_iter()
            .map(|(layout, label)| LayoutChoice {
                layout,
                label,
                selected: state.layout == layout,
                action: Action::Layout(layout),
            })
            .collect()
        } else {
            Vec::new()
        },
        body,
        decide: decide(env),
    }
}
