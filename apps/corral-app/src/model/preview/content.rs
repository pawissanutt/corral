mod board;
mod builds;
mod plans;
mod question;
mod review;
mod scale;

use board::TASKS;
pub(super) use board::{rail, save_task, stages, task};
use builds::builds;
use plans::plans;
pub(super) use plans::refresh_plans;
use question::question;
use review::reviews;
use scale::scale;

use super::{put, stamp};
use crate::model::{board::Mark, daemon::*};
use corral_protocol::{
    board::{RailGlyph, RailStep},
    detail::*,
    rows::*,
};

pub(super) fn populate(feed: &mut Feed) {
    for &(id, title, plan, group, now) in TASKS {
        let mut row = task(feed, id, title, plan, group, now);
        if id == "T-867" {
            row.rail[1].mark = Mark::Stale;
            row.rail[1].sha7 = Some("5949a4b".into());
        }
        save_task(feed, row);
    }
    question(feed);
    plans(feed);
    scale(feed);
    settings(feed);
    reviews(feed);
    builds(feed);
}

pub(super) fn settings(feed: &mut Feed) {
    let choices: Vec<_> = [
        ("pi-sol xhigh", "gpt-6.1-sol", "xhigh"),
        ("pi-terra high", "gpt-5.6-terra", "high"),
        ("pi-luna", "gpt-6-luna", "low"),
        ("pi-opus high", "claude-opus-4-5", "high"),
    ]
    .into_iter()
    .map(|(name, model, effort)| ModelChoice {
        name: name.into(),
        model: model.into(),
        effort: Some(effort.into()),
        note: Some("Preview profile · no provider contacted".into()),
    })
    .collect();
    feed.settings.insert(
        "workspace".into(),
        SettingsSnapshot {
            id: "workspace".into(),
            workspace: "/preview/corral".into(),
            store: "/preview/corral/.corral".into(),
            host: "preview-workstation".into(),
            mode: OperatingMode::Assist,
            grants: vec![GrantLine {
                key: "dispatch".into(),
                enabled: false,
                scope: Some("preview".into()),
                expires_at: None,
            }],
            profiles: choices
                .iter()
                .map(|choice| ProfileChoice {
                    name: choice.name.clone(),
                    harness: "corral-pi".into(),
                    model: Some(choice.model.clone()),
                    effort: choice.effort.clone(),
                    workspace: true,
                    inherited: false,
                    references: vec![],
                    remove_refusal: None,
                    summary_refusal: None,
                })
                .collect(),
            assistant_models: choices,
            assistant_profile: Some("pi-sol xhigh".into()),
            summary_profile: None,
            listener: None,
            config_hash: "preview-config-1".into(),
            rev: 1,
        },
    );
}

fn owner_move(
    feed: &mut Feed,
    key: &str,
    task: &str,
    plan: Option<&str>,
    title: &str,
    kind: InboxKind,
    destination: AttentionDestination,
) {
    feed.inbox.insert(
        key.into(),
        InboxRow {
            id: key.into(),
            kind,
            task: Some(task.into()),
            plan: plan.map(str::to_owned),
            project: feed.board.task(task).and_then(|row| row.project.clone()),
            title: title.into(),
            from: "Preview · canonical owner move".into(),
            asked_at: Some(stamp(feed, feed.now.saturating_sub(300))),
            needed_by: None,
            blocking: true,
            state: InboxState::Open,
            question_rev: None,
            answerable: false,
            unanswerable_reason: Some("Use this record's canonical action controls".into()),
            attention: Some(AttentionMove {
                whose_move: AttentionActor::Owner,
                reason: title.into(),
                next_step: title.into(),
                command: None,
                destination,
            }),
            rev: 1,
        },
    );
}
