//! The served candidate's Diff tab: each file of the recorded base..head diff, its patch once
//! expanded (fetched as `ReviewFile`, never read from a host path), its annotation threads at
//! their lines, and the composers that open, reply to, resolve and reopen them.

use corral_protocol::cmd::annotation::AnnotationSide;
use corral_protocol::detail::{DetailBody, ReviewAnnotation, ReviewDetail, ReviewFile};

use crate::model::daemon::Feed;
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::nav::Form;
use crate::model::text::span;

use super::decide::{comment_refusal, reply_refusal, status_lines};
use super::diff::{Anchor, line_index, parse_patch};
use super::live::{Shown, file_ref, short, when};
use super::view::{
    ButtonStyle, CommentView, Composer, DiffView, FileBody, FileView, HunkView, MessageView,
    button, chip, hunk_content,
};
use super::{Action, CandidateAt, Compose, Intent, Layout, LineAt, State};

/// Where a thread or new comment sits in a file's shown lines.
#[derive(Clone, Copy)]
struct Place {
    hunk: usize,
    after: usize,
}

fn side_word(old_side: bool) -> &'static str {
    if old_side { "old line" } else { "line" }
}

/// `line 42`, `old lines 40–42`, `whole file`.
fn anchor_label(thread: &ReviewAnnotation) -> String {
    let old_side = thread.side == AnnotationSide::Old;
    match &thread.range {
        None => "whole file".into(),
        Some(range) if range.start == range.end => {
            format!("{} {}", side_word(old_side), range.end)
        }
        Some(range) => format!("{}s {}–{}", side_word(old_side), range.start, range.end),
    }
}

fn thread_view(
    state: &State,
    feed: &Feed,
    shown: &Shown,
    thread: &ReviewAnnotation,
    env: Env,
) -> CommentView {
    let id = Id::from(thread.id.as_str());
    let messages = thread
        .messages
        .iter()
        .map(|message| {
            let mut attribution = vec![span(message.by.clone()).bold()];
            let at = when(&message.at, feed);
            if !at.is_empty() {
                attribution.push(span(format!(" · {at}")));
            }
            attribution.push(span(" · on "));
            attribution.push(span(short(&thread.head).to_owned()).mono());
            MessageView {
                attribution,
                text: message.text.clone(),
            }
        })
        .collect();
    let mut chips = Vec::new();
    if thread.resolved {
        chips.push(chip("resolved", Hue::Ok));
    }
    if thread.stale {
        chips.push(chip("older candidate", Hue::Held));
    }
    let concerns = |intent: &Intent| match intent {
        Intent::Reply { thread, .. } | Intent::Resolve { thread } | Intent::Reopen { thread } => {
            *thread == id
        }
        _ => false,
    };
    let refusal = reply_refusal(feed, env);
    let (composer, actions) = if thread.resolved {
        (
            None,
            vec![button(
                "Reopen",
                Action::Reopen {
                    thread: id.clone(),
                    revision: thread.revision,
                },
                ButtonStyle::Quiet,
            )],
        )
    } else {
        let candidate = CandidateAt {
            task: shown.id().clone(),
            candidate: corral_protocol::detail::ReviewCandidate {
                run: thread.run.clone(),
                base: thread.base.clone(),
                head: thread.head.clone(),
            },
        };
        (
            Some(Composer {
                draft: candidate.draft(&format!("reply:{id}")),
                target: Compose::Reply {
                    candidate,
                    thread: id.clone(),
                    revision: thread.revision,
                },
                placeholder: "Reply as owner…",
                label: "Reply to this thread",
                send: "Reply",
                cancel: None,
                refusal,
            }),
            vec![button(
                "Resolve",
                Action::Resolve {
                    thread: id.clone(),
                    revision: thread.revision,
                },
                ButtonStyle::Quiet,
            )],
        )
    };
    CommentView {
        key: thread.id.clone(),
        anchor: Some(anchor_label(thread)),
        messages,
        chips,
        composer,
        actions,
        status: status_lines(state, feed, shown.id(), concerns),
    }
}

fn new_comment(
    state: &State,
    feed: &Feed,
    shown: &Shown,
    candidate: &CandidateAt,
    at: &LineAt,
    env: Env,
) -> CommentView {
    let target = at.clone();
    let concerns =
        move |intent: &Intent| matches!(intent, Intent::Comment { at, .. } if *at == target);
    CommentView {
        key: draft_key(candidate, at),
        anchor: Some(format!("{} {}", side_word(at.old_side), at.line)),
        messages: Vec::new(),
        chips: Vec::new(),
        composer: Some(Composer {
            draft: draft_key(candidate, at),
            target: Compose::New {
                candidate: candidate.clone(),
                at: at.clone(),
            },
            placeholder: "Comment on this line as owner…",
            label: "New line comment",
            send: "Comment",
            cancel: Some(button("Cancel", Action::CancelCompose, ButtonStyle::Quiet)),
            refusal: if !shown.matches(candidate) {
                Some("This composer belongs to an older candidate. Reopen a line on the current patch.".into())
            } else {
                comment_refusal(feed, shown, env).or_else(|| patch_refusal(feed, shown, &at.path))
            },
        }),
        actions: Vec::new(),
        status: status_lines(state, feed, shown.id(), concerns),
    }
}

/// The task/candidate-scoped draft key of a new comment.
pub(super) fn draft_key(candidate: &CandidateAt, at: &LineAt) -> String {
    candidate.draft(&format!(
        "new:{}:{}{}",
        at.path,
        if at.old_side { "o" } else { "n" },
        at.line
    ))
}

/// Line actions must be bound to a current patch carrying the same immutable identity.
pub(super) fn patch_refusal(feed: &Feed, shown: &Shown, path: &str) -> Option<String> {
    let current = feed.detail(&file_ref(shown.id(), path)).is_some_and(|cache| {
        !cache.stale && !cache.loading && cache.error.is_none()
            && matches!(cache.body(), Some(DetailBody::ReviewFile(page))
                if page.path == path && page.candidate.as_ref().is_some_and(|candidate| shown.matches_candidate(candidate)))
    });
    (!current).then(|| {
        "The patch is last known or belongs to another candidate. Refresh it before commenting."
            .into()
    })
}

fn collapsed_label(file: &ReviewFile, threads: usize) -> String {
    let mut label = format!("{} changed lines · expand", file.added + file.deleted);
    if threads > 0 {
        label.push_str(&format!(
            " · {threads} thread{}",
            if threads == 1 { "" } else { "s" }
        ));
    }
    label
}

fn file_view(
    state: &State,
    feed: &Feed,
    shown: &Shown,
    file: &ReviewFile,
    threads: &[&ReviewAnnotation],
    layout: Layout,
    env: Env,
) -> FileView {
    let id = shown.id();
    let heading = vec![
        span(file.path.clone()).mono(),
        span(format!(" +{}", file.added)).hue(Hue::Ok),
        span(format!(" −{}", file.deleted)).hue(Hue::Fail),
    ];
    let expanded = state.expanded.contains(&(id.clone(), file.path.clone()));
    let mut view = FileView {
        path: file.path.clone(),
        heading,
        illustrative: None,
        ask: None,
        collapse: None,
        discussions: Vec::new(),
        loose: Vec::new(),
        annotatable: false,
        candidate: None,
        body: FileBody::Collapsed(button(
            collapsed_label(file, threads.len()),
            Action::Expand(file.path.clone()),
            ButtonStyle::Ghost,
        )),
    };
    if !expanded {
        return view;
    }
    view.collapse = Some(button(
        "Collapse",
        Action::Collapse(file.path.clone()),
        ButtonStyle::Quiet,
    ));
    let cache = feed.detail(&file_ref(id, &file.path));
    let patch = cache.and_then(|cache| match cache.body() {
        Some(DetailBody::ReviewFile(page)) => Some(page),
        _ => None,
    });
    let Some(patch) = patch else {
        view.body = match cache.and_then(|cache| cache.error.as_ref()) {
            Some(error) => FileBody::Unavailable(format!(
                "Patch unavailable: {}. Collapse and expand to try again.",
                error.message
            )),
            None => FileBody::Loading("Loading this file's patch…".into()),
        };
        return view;
    };
    let parsed = parse_patch(&patch.patch, true);
    view.ask = Some(button(
        "Ask about this file",
        Action::AskAbout {
            path: file.path.clone(),
            hunk: parsed.hunks.first().map(|hunk| hunk.header.clone()),
        },
        ButtonStyle::Quiet,
    ));
    let patch_note = patch_refusal(feed, shown, &file.path);
    view.annotatable = patch_note.is_none() && comment_refusal(feed, shown, env).is_none();
    view.candidate = patch_note.is_none().then(|| shown.identity()).flatten();
    let mut placed: Vec<Vec<Anchor>> = vec![Vec::new(); parsed.hunks.len()];
    let place = |old_side: bool, line: u32| {
        parsed.hunks.iter().enumerate().find_map(|(hunk, parsed)| {
            line_index(&parsed.lines, line, old_side).map(|after| Place { hunk, after })
        })
    };
    for thread in threads {
        let discussion = view.discussions.len();
        view.discussions
            .push(thread_view(state, feed, shown, thread, env));
        let at = thread
            .range
            .as_ref()
            .filter(|_| !thread.stale && patch_note.is_none())
            .and_then(|range| place(thread.side == AnnotationSide::Old, range.end));
        match at {
            Some(at) => placed[at.hunk].push(Anchor {
                after: at.after,
                discussion,
            }),
            None => view.loose.push(discussion),
        }
    }
    let composing = state
        .composing
        .as_ref()
        .filter(|(candidate, at)| candidate.task == *id && at.path == file.path);
    let mut extra: Vec<(CommentView, &LineAt, bool)> = Vec::new();
    if let Some((candidate, at)) = composing {
        extra.push((
            new_comment(state, feed, shown, candidate, at, env),
            at,
            patch_note.is_none() && shown.matches(candidate),
        ));
    }
    // A sent comment stays at its line with its status until corrald's thread replaces it.
    for sent in state.sent.iter().filter(|sent| sent.task == *id) {
        let Intent::Comment { at, text } = &sent.intent else {
            continue;
        };
        if at.path != file.path || composing.is_some_and(|(_, current)| current == at) {
            continue;
        }
        let status = status_lines(state, feed, id, |intent| *intent == sent.intent);
        if status.is_empty() {
            continue;
        }
        let comment = CommentView {
            key: sent.key.to_string(),
            anchor: Some(format!("{} {}", side_word(at.old_side), at.line)),
            messages: vec![MessageView {
                attribution: vec![span("you").bold(), span(" · not yet recorded by corrald")],
                text: text.clone(),
            }],
            chips: Vec::new(),
            composer: None,
            actions: Vec::new(),
            status,
        };
        extra.push((
            comment,
            at,
            patch_note.is_none()
                && sent
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| shown.matches(candidate)),
        ));
    }
    for (comment, at, same_candidate) in extra {
        let discussion = view.discussions.len();
        view.discussions.push(comment);
        match same_candidate
            .then(|| place(at.old_side, at.line))
            .flatten()
        {
            Some(spot) => placed[spot.hunk].push(Anchor {
                after: spot.after,
                discussion,
            }),
            None => view.loose.push(discussion),
        }
    }
    let mut notes = parsed.notes;
    if let Some(note) = patch_note {
        notes.insert(0, note);
    }
    if parsed.hunks.is_empty() && notes.is_empty() {
        notes.push("No textual changes are served for this file.".into());
    }
    view.body = FileBody::Hunks {
        hunks: parsed
            .hunks
            .into_iter()
            .zip(placed)
            .map(|(hunk, anchors)| HunkView {
                header: hunk.header,
                content: hunk_content(hunk.lines, &anchors, layout),
            })
            .collect(),
        notes,
        tail: patch.truncated.then(|| {
            "corrald stops this patch at its size bound; the rest of the file's changes are not served to the app.".into()
        }),
    };
    view
}

/// The Diff tab of a served candidate.
pub(super) fn diff(
    state: &State,
    feed: &Feed,
    shown: &Shown,
    review: &ReviewDetail,
    layout: Layout,
    env: Env,
) -> DiffView {
    let files = review
        .files
        .iter()
        .map(|file| {
            let threads: Vec<&ReviewAnnotation> = review
                .annotations
                .iter()
                .filter(|thread| thread.path == file.path)
                .collect();
            file_view(state, feed, shown, file, &threads, layout, env)
        })
        .collect();
    DiffView {
        layout,
        notice: (env.form == Form::Phone)
            .then_some("Unified on phones; side by side needs ≥ 900 pt."),
        files,
        empty: review
            .files
            .is_empty()
            .then(|| "corrald reports no changed files for this candidate.".into()),
    }
}
