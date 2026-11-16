//! A build's detail from corrald's `BuildDetail`: the head with the override called out, then
//! the sections in their fixed order (outcome and intent; slices; verification; review rounds
//! and findings; agent messages; owner decisions; usage; timeline). A section with no entries
//! is left out; one corrald shortened says how many of how many it shows.

use corral_protocol::detail::{
    BuildDetail, BuildDiff, BuildFinding, BuildOutcome, BuildReview, BuildSlice, BuildUsage,
};

use super::wording::{self, Outcome};
use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon::{Feed, Link};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};
use crate::model::work::when;

/// What the detail shows for one build.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    /// Its detail has not arrived yet.
    Loading,
    /// corrald could not send it; the screen offers Retry.
    Failed {
        id: Id,
        line: String,
    },
    /// corrald no longer lists the build and holds no copy of it here.
    Missing(Id),
    Ready(Box<BuildView>),
}

/// A served build, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildView {
    pub id: Id,
    pub title: String,
    /// Chips before the id: the outcome, the session kind when it is not a build session.
    pub chips: Vec<(String, Hue)>,
    /// The project and the last update, after the id.
    pub meta: String,
    /// A retained copy's notice.
    pub stale: Option<String>,
    /// The override's callout, set apart above the sections.
    pub alert: Option<Rich>,
    pub sections: Vec<Section>,
}

/// One section, in the fixed order.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// For test ids: `build-section-<key>`.
    pub key: &'static str,
    pub heading: &'static str,
    /// How many entries, or "showing n of m …" when corrald shortened the list.
    pub note: Option<String>,
    pub blocks: Vec<Block>,
}

/// A piece of a section.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Label and value rows.
    Facts(Vec<(String, Rich)>),
    /// The owner's own words, with when they were said.
    Quote { at: String, text: String },
    /// Marked entries; `nested` ones belong to the entry before them (a round's findings).
    Lines { lines: Vec<Line>, nested: bool },
    /// Figures under a header; `end` sets a column against its trailing edge.
    Table {
        header: Vec<&'static str>,
        end: Vec<bool>,
        rows: Vec<Vec<String>>,
    },
    /// A quiet aside: what a shortened list leaves out, what a figure means.
    Note(String),
}

/// One marked entry: a slice, gate attempt, round, finding, message, decision or phase.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub mark: Mark,
    pub hue: Hue,
    pub chips: Vec<(String, Hue)>,
    pub head: Rich,
    pub body: Vec<Rich>,
}

impl Line {
    fn new(mark: Mark, hue: Hue, head: Rich) -> Line {
        Line {
            mark,
            hue,
            chips: vec![],
            head,
            body: vec![],
        }
    }
}

/// The detail of one served build, its cache state included.
pub fn detail_of(feed: &Feed, id: &Id) -> Detail {
    let cache = feed.detail(&super::build_ref(id));
    let body = super::build_detail(feed, id);
    match (body, cache) {
        (Some(build), cache) => {
            let stale = match cache.and_then(|cache| cache.error.as_ref()) {
                Some(error) => Some(format!(
                    "Showing the last build received · {}",
                    error.message
                )),
                None if feed.link == Link::Down || cache.is_some_and(|cache| cache.stale) => {
                    Some("Showing the last build received; corrald is not current.".into())
                }
                None => None,
            };
            Detail::Ready(Box::new(view(feed, build, stale)))
        }
        (None, Some(cache)) if !cache.loading && cache.error.is_some() => Detail::Failed {
            id: id.clone(),
            line: format!(
                "Could not load build {id}: {}",
                cache
                    .error
                    .as_ref()
                    .map_or("", |error| error.message.as_str())
            ),
        },
        (None, _) if !feed.builds.contains_key(id.as_str()) => Detail::Missing(id.clone()),
        (None, _) => Detail::Loading,
    }
}

fn view(feed: &Feed, build: &BuildDetail, stale: Option<String>) -> BuildView {
    let outcome = Outcome::of(
        &build.outcome.state,
        build.outcome.override_reason.as_deref(),
    );
    let mut chips = vec![(outcome.label().to_owned(), outcome.hue())];
    if build.kind != "build" {
        chips.push((format!("{} session", build.kind), Hue::Neutral));
    }
    let updated = feed
        .builds
        .get(build.id.as_str())
        .map(|row| when(&row.updated, feed))
        .filter(|at| !at.is_empty());
    let meta = [
        build.project.clone(),
        updated.map(|at| format!("updated {at}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let sections = [
        Some(outcome_section(feed, build, outcome)),
        slices(feed, build),
        verification(feed, build),
        reviews(feed, build),
        messages(feed, build),
        decisions(feed, build),
        usage(build),
        timeline(feed, build),
    ]
    .into_iter()
    .flatten()
    .collect();
    BuildView {
        id: Id::from(&build.id),
        title: build.title.clone(),
        chips,
        meta,
        stale,
        alert: build
            .outcome
            .override_reason
            .as_deref()
            .filter(|_| outcome == Outcome::Override)
            .map(wording::override_line),
        sections,
    }
}

/// `14:02–14:09`, or the one end that is known.
fn span_of(feed: &Feed, start: Option<&str>, end: Option<&str>) -> Option<String> {
    let start = start.map(|at| when(at, feed)).filter(|at| !at.is_empty());
    let end = end.map(|at| when(at, feed)).filter(|at| !at.is_empty());
    match (start, end) {
        (Some(start), Some(end)) => Some(format!("{start}–{end}")),
        (Some(start), None) => Some(format!("since {start}")),
        (None, Some(end)) => Some(format!("ended {end}")),
        (None, None) => None,
    }
}

fn mono(text: impl Into<String>) -> Rich {
    vec![span(text).mono()]
}

fn diff_line(diff: &BuildDiff) -> Rich {
    vec![
        span(format!(
            "{} · {} · ",
            wording::count(diff.commits, "commit", "commits"),
            wording::count(diff.files, "file", "files"),
        )),
        span(format!("+{}", diff.added)).hue(Hue::Ok).mono(),
        span(" "),
        span(format!("−{}", diff.removed)).hue(Hue::Fail).mono(),
    ]
}

fn outcome_facts(
    feed: &Feed,
    build: &BuildDetail,
    out: &BuildOutcome,
    outcome: Outcome,
) -> Vec<(String, Rich)> {
    let mut facts = vec![(
        "Outcome".to_owned(),
        [
            vec![span(outcome.label()).bold().hue(outcome.hue())],
            out.at
                .as_deref()
                .map(|at| when(at, feed))
                .filter(|at| !at.is_empty())
                .map(|at| vec![span(format!(" · {at}")).dim()])
                .unwrap_or_default(),
        ]
        .concat(),
    )];
    if let Some(landed) = &out.landed {
        let mut value = mono(wording::short(landed));
        if let Some(base) = &out.base_branch {
            value.push(span(" on "));
            value.push(span(base.as_str()).mono());
        }
        facts.push(("Landed".into(), value));
    }
    if let Some(head) = &out.head {
        facts.push(("Head".into(), mono(wording::short(head))));
    }
    match (&build.branch, &build.base) {
        (Some(branch), Some(base)) => facts.push((
            "Branch".into(),
            vec![
                span(branch.as_str()).mono(),
                span(" from "),
                span(base.as_str()).mono(),
            ],
        )),
        (Some(branch), None) => facts.push(("Branch".into(), mono(branch.as_str()))),
        (None, Some(base)) => facts.push(("Base".into(), mono(base.as_str()))),
        (None, None) => {}
    }
    if let Some(diff) = &build.diff {
        facts.push(("Changes".into(), diff_line(diff)));
    }
    facts
}

fn outcome_section(feed: &Feed, build: &BuildDetail, outcome: Outcome) -> Section {
    let mut blocks = vec![Block::Facts(outcome_facts(
        feed,
        build,
        &build.outcome,
        outcome,
    ))];
    blocks.extend(build.intent.iter().map(|said| Block::Quote {
        at: when(&said.at, feed),
        text: said.text.clone(),
    }));
    if build.intent.is_empty() && build.intent_total == 0 {
        blocks.push(Block::Note(
            "No prompt of the owner's is recorded for this build.".into(),
        ));
    }
    blocks
        .extend(wording::shown(build.intent.len(), build.intent_total, "prompts").map(Block::Note));
    if !build.commits.is_empty() {
        blocks.push(Block::Lines {
            lines: build
                .commits
                .iter()
                .map(|commit| {
                    let mut head = mono(wording::short(&commit.sha));
                    head.push(span(format!(" {}", commit.subject)));
                    if let Some(slice) = &commit.slice {
                        head.push(span(format!(" · {slice}")).dim());
                    }
                    Line::new(Mark::Done, Hue::Neutral, head)
                })
                .collect(),
            nested: false,
        });
    }
    blocks.extend(
        wording::shown(
            build.commits.len(),
            build.commits_total,
            "commits, the newest",
        )
        .map(Block::Note),
    );
    Section {
        key: "outcome",
        heading: "Outcome and intent",
        note: None,
        blocks,
    }
}

/// A section for a list with entries (or entries the record holds that corrald left out).
fn listed(
    key: &'static str,
    heading: &'static str,
    len: usize,
    total: u64,
    noun: &str,
    blocks: Vec<Block>,
) -> Option<Section> {
    if len == 0 && total == 0 {
        return None;
    }
    let mut blocks = blocks;
    if len == 0 {
        blocks.push(Block::Note(format!(
            "The record holds {total} {noun}; corrald left all of them out of this read."
        )));
    }
    Some(Section {
        key,
        heading,
        note: wording::note(len, total, noun),
        blocks,
    })
}

fn slice_line(feed: &Feed, slice: &BuildSlice) -> Line {
    let (mark, hue, state) = wording::slice_state(&slice.state);
    let mut head = mono(slice.id.as_str());
    head.push(span(format!(" {}", slice.title)).bold());
    let mut line = Line::new(mark, hue, head);
    line.chips.push((slice.kind.clone(), Hue::Neutral));
    let model = match &slice.effort {
        Some(effort) => format!("{} {effort}", slice.model),
        None => slice.model.clone(),
    };
    let mut facts = vec![
        span(format!("{model} · ")).dim(),
        span(state).hue(hue),
        span(format!(
            " · {} · {} · {}",
            wording::count(slice.turns, "turn", "turns"),
            wording::tokens(slice.tokens),
            wording::cost(slice.cost_usd),
        ))
        .dim(),
    ];
    if let Some(range) = span_of(feed, slice.started.as_deref(), slice.ended.as_deref()) {
        facts.push(span(format!(" · {range}")).dim());
    }
    line.body.push(facts);
    if !slice.outside.is_empty() {
        line.body.push(vec![
            span("Edited outside its ownership: ").bold().hue(Hue::Held),
            span(slice.outside.join(", ")).mono(),
        ]);
    }
    if let Some(report) = slice
        .report
        .as_deref()
        .map(str::trim)
        .filter(|report| !report.is_empty())
    {
        line.body.push(vec![span(report)]);
    }
    line
}

fn slices(feed: &Feed, build: &BuildDetail) -> Option<Section> {
    let lines: Vec<Line> = build
        .slices
        .iter()
        .map(|slice| slice_line(feed, slice))
        .collect();
    let blocks = if lines.is_empty() {
        vec![]
    } else {
        vec![Block::Lines {
            lines,
            nested: false,
        }]
    };
    listed(
        "slices",
        "Slices",
        build.slices.len(),
        build.slices_total,
        "slices",
        blocks,
    )
}

fn verification(feed: &Feed, build: &BuildDetail) -> Option<Section> {
    let lines: Vec<Line> = build
        .verification
        .iter()
        .map(|gate| {
            let (mark, hue, result) = wording::gate_result(&gate.result);
            let mut head = vec![span(format!("Attempt {} · {} · ", gate.attempt, gate.tier))];
            head.extend(mono(wording::short(&gate.head)));
            let mut line = Line::new(mark, hue, head);
            let mut facts = vec![span(result).hue(hue)];
            if let Some(range) = span_of(feed, Some(&gate.started), Some(&gate.ended)) {
                facts.push(span(format!(" · {range}")).dim());
            }
            line.body.push(facts);
            if let Some(command) = &gate.failed_command {
                line.body.push(vec![
                    span("Failed at ").hue(Hue::Fail),
                    span(command.as_str()).mono(),
                ]);
            }
            line
        })
        .collect();
    let blocks = if lines.is_empty() {
        vec![]
    } else {
        vec![Block::Lines {
            lines,
            nested: false,
        }]
    };
    listed(
        "verification",
        "Verification",
        build.verification.len(),
        build.verification_total,
        "gate attempts",
        blocks,
    )
}

fn finding_line(finding: &BuildFinding) -> Line {
    let (mark, hue, resolution) = match finding.resolution.as_ref().map(|r| r.state.as_str()) {
        Some("fixed") => (Mark::Done, Hue::Ok, "fixed"),
        Some("declined") => (Mark::Na, Hue::Neutral, "declined"),
        Some("open") | None => (Mark::Next, Hue::Held, "open"),
        Some(_) => (Mark::Na, Hue::Neutral, "resolution not known to this app"),
    };
    let mut line = Line::new(mark, hue, vec![span(finding.text.as_str())]);
    line.chips.push(wording::severity(&finding.severity));
    let mut facts = vec![span(resolution).hue(hue)];
    if let Some(resolved) = &finding.resolution {
        if let Some(commit) = &resolved.commit {
            facts.push(span(" in "));
            facts.push(span(wording::short(commit)).mono());
        }
        if let Some(reason) = &resolved.reason {
            facts.push(span(format!(": {reason}")));
        }
    }
    if let Some(path) = &finding.path {
        facts.push(span(" · ").dim());
        let place = match finding.line {
            Some(at) => format!("{path}:{at}"),
            None => path.clone(),
        };
        facts.push(span(place).mono());
    }
    line.body.push(facts);
    line
}

fn round_line(feed: &Feed, review: &BuildReview) -> Line {
    let (mark, hue, verdict) = wording::verdict(&review.verdict);
    let head = vec![
        span(format!("Round {} · ", review.round)).bold(),
        span(verdict).bold().hue(hue),
        span(format!(" · {} · {}", review.reviewer, review.model)).dim(),
    ];
    let mut line = Line::new(mark, hue, head);
    let mut facts = vec![span("reviewed ").dim()];
    facts.extend(mono(wording::short(&review.head)));
    let at = when(&review.at, feed);
    if !at.is_empty() {
        facts.push(span(format!(" · {at}")).dim());
    }
    facts.push(
        span(format!(
            " · {}",
            wording::count(
                u64::try_from(review.findings.len()).unwrap_or(u64::MAX),
                "finding",
                "findings"
            )
        ))
        .dim(),
    );
    line.body.push(facts);
    if let Some(summary) = review
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        line.body.push(vec![span(summary)]);
    }
    line
}

fn reviews(feed: &Feed, build: &BuildDetail) -> Option<Section> {
    let mut blocks = Vec::new();
    for review in &build.reviews {
        blocks.push(Block::Lines {
            lines: vec![round_line(feed, review)],
            nested: false,
        });
        if !review.findings.is_empty() {
            blocks.push(Block::Lines {
                lines: review.findings.iter().map(finding_line).collect(),
                nested: true,
            });
        }
    }
    listed(
        "reviews",
        "Review rounds",
        build.reviews.len(),
        build.reviews_total,
        "rounds",
        blocks,
    )
}

fn messages(feed: &Feed, build: &BuildDetail) -> Option<Section> {
    let lines: Vec<Line> = build
        .messages
        .iter()
        .map(|message| {
            let (mark, hue, words) = wording::delivery(&message.delivery);
            let mut head = vec![
                span(message.from.as_str()).mono(),
                span(" → "),
                span(message.to.as_str()).mono(),
            ];
            let at = when(&message.at, feed);
            if !at.is_empty() {
                head.push(span(format!(" · {at}")).dim());
            }
            let mut line = Line::new(mark, hue, head);
            line.body.push(vec![span(message.text.as_str())]);
            if let Some(words) = words {
                line.body.push(vec![span(words).hue(hue)]);
            }
            line
        })
        .collect();
    let blocks = if lines.is_empty() {
        vec![]
    } else {
        vec![Block::Lines {
            lines,
            nested: false,
        }]
    };
    listed(
        "messages",
        "Agent messages",
        build.messages.len(),
        build.messages_total,
        "messages",
        blocks,
    )
}

fn decisions(feed: &Feed, build: &BuildDetail) -> Option<Section> {
    let lines: Vec<Line> = build
        .decisions
        .iter()
        .map(|decision| {
            let (kind, hue) = wording::decision(&decision.kind);
            let mut line = Line::new(Mark::Done, hue, vec![span(decision.text.as_str())]);
            line.chips.push((kind.to_owned(), hue));
            let mut facts = Vec::new();
            if let Some(answer) = &decision.answer {
                facts.push(span("Answer: ").bold());
                facts.push(span(answer.as_str()));
            }
            let at = when(&decision.at, feed);
            if !at.is_empty() {
                facts.push(
                    span(if facts.is_empty() {
                        at
                    } else {
                        format!(" · {at}")
                    })
                    .dim(),
                );
            }
            if !facts.is_empty() {
                line.body.push(facts);
            }
            line
        })
        .collect();
    let blocks = if lines.is_empty() {
        vec![]
    } else {
        vec![Block::Lines {
            lines,
            nested: false,
        }]
    };
    listed(
        "decisions",
        "Owner decisions",
        build.decisions.len(),
        build.decisions_total,
        "decisions",
        blocks,
    )
}

fn usage_row(label: &str, usage: &BuildUsage) -> Vec<String> {
    vec![
        label.to_owned(),
        crate::model::usage::tokens(usage.input),
        crate::model::usage::tokens(usage.output),
        wording::cost(usage.cost_usd),
    ]
}

/// The usage table, left out while nothing was used.
fn usage(build: &BuildDetail) -> Option<Section> {
    let usage = &build.usage;
    let total = &usage.total;
    if total.input == 0 && total.output == 0 && total.cost_usd.is_none() {
        return None;
    }
    Some(Section {
        key: "usage",
        heading: "Usage",
        note: None,
        blocks: vec![
            Block::Table {
                header: vec!["", "Input", "Output", "Cost"],
                end: vec![false, true, true, true],
                rows: vec![
                    usage_row("This session", &usage.parent),
                    usage_row("Child agents", &usage.children),
                    usage_row("Reviewers", &usage.reviewers),
                    usage_row("Total", &usage.total),
                ],
            },
            Block::Note(
                "Tokens in and out; cost is the API-equivalent price, unknown where a model has \
                 no price."
                    .into(),
            ),
        ],
    })
}

fn timeline(feed: &Feed, build: &BuildDetail) -> Option<Section> {
    let lines: Vec<Line> = build
        .timeline
        .iter()
        .map(|entry| {
            let at = when(&entry.at, feed);
            let mut head = Vec::new();
            if !at.is_empty() {
                head.push(span(format!("{at} ")).mono().dim());
            }
            head.push(span(wording::phase(&entry.phase)).bold());
            let mut line = Line::new(Mark::Done, Hue::Neutral, head);
            if let Some(detail) = &entry.detail {
                line.body.push(vec![span(detail.as_str())]);
            }
            line
        })
        .collect();
    let blocks = if lines.is_empty() {
        vec![]
    } else {
        vec![Block::Lines {
            lines,
            nested: false,
        }]
    };
    listed(
        "timeline",
        "Timeline",
        build.timeline.len(),
        build.timeline_total,
        "entries",
        blocks,
    )
}
