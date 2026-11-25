//! The `/` menu over the composer: while the draft is one line starting with `/`, the catalog
//! rows whose terminal command starts with the typed name (or, before any argument, whose label
//! holds it), then the host's own commands (`DetailRef::SessionCommands`: extension commands,
//! prompt templates, skills and the host's commands the catalog does not run itself). A catalog
//! row runs its action (Compact takes what follows the name as its instructions); a host
//! command is sent as the typed text through the normal send, where the host runs it. One whose
//! argument is required fills the draft with its name instead, so the argument can be typed.
//! Commands the app does not carry stay listed with why. Esc closes the menu and keeps the text;
//! a `/text` that matches nothing is sent as typed.

use corral_protocol::detail::{
    CommandSource, DetailBody, DetailRef, SessionCommands, SlashCommand,
};
use corral_protocol::session_host::SessionAction;
use corral_protocol::session_state::SendPolicy;

use crate::model::hue::Hue;
use crate::model::session::{Action, Can, Cx, ReadState, SessionActions};

use super::CommandAction;

/// At most this many rows show.
pub const SHOWN: usize = 8;

/// The menu as it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SlashMenu {
    pub rows: Vec<SlashRow>,
    /// The highlighted row: what Enter runs.
    pub at: Option<usize>,
    /// Matched rows beyond those shown.
    pub more: usize,
    /// Where the read of the host's commands stands, while it says something.
    pub note: Option<(Hue, String)>,
    /// Esc: close the menu, keep the text.
    pub close: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlashRow {
    /// `cmd-<slug>`.
    pub id: String,
    pub label: String,
    /// `/compact`.
    pub command: String,
    /// `[instructions]`, `<view>`.
    pub args: Option<String>,
    /// `extension`, `prompt`, `skill`; none for the app's own rows.
    pub source: Option<&'static str>,
    /// Listed muted with this; does not run.
    pub unavailable: Option<String>,
    /// What choosing it runs, in order.
    pub run: Vec<Action>,
}

/// The typed name (lower-cased, without the `/`) and what follows it, while the draft is a one
/// line `/` command.
fn typed(draft: &str) -> Option<(String, Option<String>)> {
    let rest = draft.strip_prefix('/')?;
    if rest.contains('\n') {
        return None;
    }
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name, Some(args.trim().to_owned())),
        None => (rest, None),
    };
    Some((name.to_lowercase(), args))
}

fn source(source: CommandSource) -> Option<&'static str> {
    match source {
        CommandSource::Extension => Some("extension"),
        CommandSource::Prompt => Some("prompt"),
        CommandSource::Skill => Some("skill"),
        CommandSource::Host | CommandSource::Other => None,
    }
}

fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionCommands {
        id: cx.id.to_string(),
    }
}

fn served<'f>(cx: &Cx<'f>) -> Option<&'f SessionCommands> {
    match cx.feed.detail(&target(cx))?.body()? {
        DetailBody::SessionCommands(commands) => Some(commands),
        _ => None,
    }
}

/// Whether the command `command` (with its `/`) matches the typed name: exactly once an
/// argument follows, else by its start.
fn named(command: &str, name: &str, args: bool) -> bool {
    let command = command.trim_start_matches('/').to_lowercase();
    if args {
        command == name
    } else {
        command.starts_with(name)
    }
}

fn clear(cx: &Cx<'_>) -> Action {
    Action::Draft {
        session: cx.id.clone(),
        text: String::new(),
    }
}

/// Why a message cannot be sent now, as the host declares.
fn refused(cx: &Cx<'_>) -> Option<String> {
    match cx.can("send") {
        Can::Unavailable(why) => Some(why),
        Can::Absent => Some("this session takes no messages".into()),
        Can::Yes | Can::Unknown => None,
    }
}

/// A host command sent as typed, or put into the draft while its argument is still required.
fn host_command(cx: &Cx<'_>, command: &SlashCommand, args: Option<&str>) -> SlashRow {
    let name = format!("/{}", command.name);
    let required = command
        .args_hint
        .as_deref()
        .is_some_and(|hint| hint.trim_start().starts_with('<'));
    let run = match args.filter(|args| !args.is_empty()) {
        None if required => vec![Action::Draft {
            session: cx.id.clone(),
            text: format!("{name} "),
        }],
        args => vec![Action::Send {
            session: cx.id.clone(),
            text: match args {
                Some(args) => format!("{name} {args}"),
                None => name.clone(),
            },
            policy: SendPolicy::Steer,
            attachments: Vec::new(),
            restore: None,
        }],
    };
    let changing_model = super::picker::changing(cx)
        && run
            .iter()
            .any(|action| matches!(action, Action::Send { .. }));
    SlashRow {
        id: format!("cmd-host-{}", command.name.replace(':', "-")),
        label: if command.description.trim().is_empty() {
            name.clone()
        } else {
            command.description.clone()
        },
        command: name,
        args: command.args_hint.clone(),
        source: source(command.source),
        unavailable: if changing_model {
            Some("Waiting for the model change to finish".into())
        } else {
            refused(cx)
        },
        run,
    }
}

/// The `/` menu of the session `cx` reads over its `catalog`; none while the draft is not a `/`
/// command, after Esc closed it on this text, or while nothing matches and nothing is read.
pub fn slash(cx: &Cx<'_>, catalog: &SessionActions) -> Option<SlashMenu> {
    let draft = &cx.ui.draft;
    let (name, args) = typed(draft)?;
    if cx.ui.picker.slash_closed.as_deref() == Some(draft.as_str()) {
        return None;
    }
    let argued = args.is_some();
    let mut rows: Vec<SlashRow> = Vec::new();
    for found in catalog.rows() {
        let matched = match found.command {
            Some(command) => named(command, &name, argued),
            None => !argued && !name.is_empty() && found.label.to_lowercase().contains(&name),
        };
        if !matched {
            continue;
        }
        let action = match (&found.action, args.as_deref()) {
            (
                Action::Command(CommandAction::Host {
                    session,
                    action: SessionAction::Compact { .. },
                    what,
                }),
                Some(instructions),
            ) if !instructions.is_empty() => Action::Command(CommandAction::Host {
                session: session.clone(),
                action: SessionAction::Compact {
                    instructions: Some(instructions.to_owned()),
                },
                what: *what,
            }),
            (action, _) => action.clone(),
        };
        rows.push(SlashRow {
            id: format!("cmd-{}", found.id.trim_start_matches("act-")),
            label: found.label.clone(),
            command: found.command.map(str::to_owned).unwrap_or_default(),
            args: (found.id == "act-compact").then(|| "[instructions]".to_owned()),
            source: None,
            unavailable: found.unavailable.clone(),
            run: vec![clear(cx), action],
        });
    }
    let claimed: Vec<&str> = catalog
        .rows()
        .filter_map(|found| found.command)
        .map(|command| command.trim_start_matches('/'))
        .collect();
    if let Some(commands) = served(cx) {
        rows.extend(
            commands
                .commands
                .iter()
                .filter(|command| !claimed.contains(&command.name.as_str()))
                .filter(|command| named(&command.name, &name, argued))
                .map(|command| host_command(cx, command, args.as_deref())),
        );
    }
    let read = ReadState::of(cx.feed.detail(&target(cx)), cx.feed);
    let note = match read {
        ReadState::Reading if !matches!(cx.can("commands"), Can::Absent) => {
            Some((Hue::Neutral, "Reading this session's commands…".to_owned()))
        }
        ReadState::Failed(why) => Some((
            Hue::Held,
            format!("Could not read this session's commands: {why}"),
        )),
        _ => None,
    };
    if rows.is_empty() && note.is_none() {
        return None;
    }
    let more = rows.len().saturating_sub(SHOWN);
    rows.truncate(SHOWN);
    let at = (!rows.is_empty()).then(|| {
        let wanted = cx.ui.picker.slash_at.min(rows.len() - 1);
        if cx.ui.picker.slash_at == 0 {
            rows.iter()
                .position(|row| row.unavailable.is_none())
                .unwrap_or(0)
        } else {
            wanted
        }
    });
    Some(SlashMenu {
        rows,
        at,
        more,
        note,
        close: Action::Command(CommandAction::CloseMenu(cx.id.clone())),
    })
}

/// The read of the host's commands while the draft is a `/` command: once, while nothing was
/// read and the host serves them.
pub(super) fn fetches(cx: &Cx<'_>) -> Option<Action> {
    typed(&cx.ui.draft)?;
    crate::model::session::tabs::load(cx, Ok(target(cx)), Some("commands"))
}
