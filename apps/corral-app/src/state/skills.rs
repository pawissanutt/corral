//! Source-bound skill requests and replies, independent of the mounted Settings tab.
use super::AppState;
use crate::client::requests::{self, ClientRequest};
use crate::model::{
    Id,
    settings::{
        editor::Part,
        skills::{self, Context, Editor, Request},
    },
};
use corral_protocol::config::{PiSkillGet, PiSkillPut, PiSkillToggle, PiSkillsRequest};
use nsh_core::prelude::*;

pub(crate) fn context(app: AppState) -> Option<Context> {
    let store = app.feed.with(|feed| feed.store.clone())?;
    let project = app.project_scope().project().map(ToString::to_string);
    Some((store, project))
}

pub(crate) fn edit(app: AppState, ctx: &Context, update: impl FnOnce(&mut Editor)) {
    app.settings
        .update(|state| update(state.skills.entry(ctx.clone()).or_default()));
}

fn send(app: AppState, ctx: &Context, action: impl FnOnce(&mut Editor) -> Option<Request>) {
    if untracked(|| context(app).as_ref() != Some(ctx))
        || app
            .settings
            .with_untracked(|state| state.config.part != Part::Skills)
        || app.feed.with_untracked(skills::unavailable).is_some()
    {
        return;
    }
    let request_id = requests::new_id();
    let fixture = app.automation_preview();
    app.settings.update(|state| {
        let editor = state.skills.entry(ctx.clone()).or_default();
        if editor.pending.is_some() {
            return;
        }
        let Some(request) = action(editor) else {
            return;
        };
        if request.too_large() {
            editor.failed(
                "Markdown is too large to send. Shorten it and try again; your draft is kept."
                    .into(),
            );
            return;
        }
        editor.problem = None;
        editor.pending = Some((Id::from(&request_id), request.clone()));
        if fixture {
            editor.settle(state.skill_fixture.reply(&request));
        } else if !requests::send(ClientRequest::PiSkills {
            request_id,
            request,
        }) {
            editor.failed("Not sent: the app's connection task is not running.".into());
        }
    });
}

pub(crate) fn list(app: AppState, ctx: &Context) {
    send(app, ctx, |editor| {
        (!editor.dirty()).then(|| {
            Request::List(PiSkillsRequest {
                project: ctx.1.clone(),
            })
        })
    });
}

pub(crate) fn get(app: AppState, ctx: &Context, id: &str) {
    send(app, ctx, |editor| {
        if editor.dirty()
            || !editor
                .catalog
                .as_ref()?
                .skills
                .iter()
                .any(|row| row.id == id)
        {
            return None;
        }
        if editor.selected.as_deref() != Some(id) {
            editor.close();
            editor.selected = Some(id.to_owned());
        }
        Some(Request::Get(PiSkillGet {
            project: ctx.1.clone(),
            id: id.to_owned(),
        }))
    });
}

pub(crate) fn save(app: AppState, ctx: &Context) {
    send(app, ctx, |editor| {
        let row = editor.row()?;
        let saved = editor.saved.as_ref()?;
        (skills::editable(row) && editor.dirty()).then(|| {
            Request::Put(PiSkillPut {
                project: ctx.1.clone(),
                id: row.id.clone(),
                text: editor.draft.clone(),
                revision: saved.revision.clone(),
            })
        })
    });
}

pub(crate) fn toggle(app: AppState, ctx: &Context) {
    send(app, ctx, |editor| {
        let row = editor.row()?;
        (!editor.dirty() && !row.required && !row.shadowed).then(|| {
            Request::Toggle(PiSkillToggle {
                project: ctx.1.clone(),
                name: row.name.clone(),
                enabled: !row.enabled,
                revision: editor
                    .catalog
                    .as_ref()
                    .map(|list| list.revision.clone())
                    .unwrap_or_default(),
            })
        })
    });
}

pub(super) fn settle(app: AppState) {
    let reason = computed(move || app.feed.with(skills::unavailable));
    effect(move || {
        let pending: Vec<_> = app.settings.with(|state| {
            state
                .skills
                .iter()
                .filter_map(|(ctx, editor)| Some((ctx.clone(), editor.pending.as_ref()?.0.clone())))
                .collect()
        });
        let replies = app.feed.with(|feed| {
            pending
                .into_iter()
                .map(|(ctx, key)| {
                    let reply = feed.operations.get(&key).filter(|op| !op.loading).cloned();
                    (ctx, reply)
                })
                .collect::<Vec<_>>()
        });
        let unavailable = reason.get();
        untracked(|| {
            for (ctx, reply) in replies {
                if let Some(reply) = reply {
                    edit(app, &ctx, |editor| editor.settle(reply));
                } else if unavailable.is_some() {
                    edit(app, &ctx, |editor| {
                        editor.failed("Connection interrupted. Read again before retrying; unsaved Markdown is kept.".into())
                    });
                }
            }
        });
    });
}
