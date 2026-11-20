//! Palette rows, their actions and the local confirm sheet's wording. Renderers display these
//! values and delegate actions to AppState; filtering and command wording stay here. Which keys
//! each destination answers, and their terminal legend, are in `keys`. While a session page is
//! on top, its action catalog leads the palette, every group of it; a row the host cannot run
//! now stays listed with its reason, dimmed, and choosing it runs nothing.

mod keys;
#[cfg(test)]
mod tests;

pub use keys::{MENU_WALK, Surface, key_legend_live, list_keys_live, menu_step, with_focus};

use crate::model::daemon::Feed;
use crate::model::session::SessionActions;
use crate::model::{
    DataState, Destination, Form, Id, Live, Rich, SettingsSection, ThemeChoice, span,
};

/// Search field placeholder shared by every palette form.
pub const PALETTE_PLACEHOLDER: &str = "Go to, open a task id, answer, accept…";

/// Keyboard help beneath the desktop palette and in the terminal palette's head. Escape first
/// clears a typed query (the command field's own behaviour), so the hint says what it does now.
pub const fn palette_hints(query_empty: bool) -> [(&'static str, &'static str); 3] {
    [
        ("↑↓", "move"),
        ("Enter", "run"),
        ("Esc", if query_empty { "close" } else { "clear" }),
    ]
}

/// Live rows of one kind the palette lists without a query, and with one.
const LIVE_IDLE: usize = 8;
const LIVE_MATCHED: usize = 40;

/// Empty-search guidance for a served record in either feed, or none when no records exist.
pub fn palette_empty(query: &str, example: Option<&str>) -> Rich {
    let mut rich = vec![span(format!("No command or task matches “{query}”."))];
    if let Some(example) = example {
        rich.extend([
            span(" Task ids work too: try "),
            span(example).mono(),
            span("."),
        ]);
    }
    rich
}

/// The example task id of the empty-search guidance.
pub fn palette_example(data: DataState, feed: &Feed) -> Option<String> {
    match data {
        DataState::Fixture | DataState::Daemon => {
            feed.board.tasks().first().map(|task| task.id.to_string())
        }
        DataState::Disconnected => None,
    }
}

/// Desktop explanation beneath the confirm sheet's title.
pub const CONFIRM_SUB: &str = "Asked once, on this device. Nothing is sent until you confirm.";

/// Heading above the operation the confirm sheet will send.
pub const CONFIRM_EFFECT_HEADING: &str = "Operation";

/// Confirmation cancellation button label.
pub const CONFIRM_CANCEL: &str = "Cancel";

/// The phone's line over Confirm: the tap itself is the confirmation; no PIN or biometric.
pub const CONFIRM_CHECK: &str = "Tapping Confirm sends it. Nothing has been sent yet.";

/// The refusal sheet's line under its title.
pub const REFUSED_SUB: &str =
    "The engine refused this. Each button below is a step that gets past it.";

/// A live command's confirming button: generic on phone, else its title's first verb.
pub fn act_confirm_label(title: &str, form: Form) -> String {
    if form == Form::Phone {
        "Confirm".into()
    } else {
        format!("Confirm {}", act_confirm_word(title))
    }
}

/// The lower-case first word of a live command's title, typed to confirm in a terminal.
pub fn act_confirm_word(title: &str) -> String {
    title
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

/// A local app command, including demo switches not represented by reducer effects.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Navigate to a destination or a Settings section.
    Go(Destination),
    /// Open a Settings section directly.
    Settings(SettingsSection),
    /// Select or push a live record.
    OpenLive(Live),
    /// Scope every page to a project and go Home: the project's page.
    Project(Id),
    /// Scope every page to all projects again.
    AllProjects,
    /// Launch the explicit assistant-only workspace preview.
    PreviewNoProjects,
    /// Run the same record action offered in the inspector and row menu.
    Work(crate::model::work::Press),
    /// Open Ask, optionally with a free-text draft.
    Ask(Option<String>),
    /// Open the New build sheet over Sessions.
    NewBuild,
    /// Start a new assistant session and open its page.
    NewAssistant,
    /// Change the persisted appearance.
    Theme(ThemeChoice),
    /// Select live transport, explicit preview, or disconnected data.
    Data(DataState),
    /// Show the global hotkey demo window.
    Hotkey,
    /// Show the phone share-sheet demo.
    Share,
    /// Show the phone's encrypted push demo.
    Lock,
    /// A row of the open session page's catalog, run by the session's reducer.
    Session(crate::model::session::Action),
}

/// One palette row in source order. `always` bypasses text matching for the free-text Ask row.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// Stable selection id, separate from the displayed label.
    pub id: String,
    /// Adjacent rows with this group share a heading.
    pub group: &'static str,
    /// Verbatim command wording.
    pub label: String,
    /// The shortcut cap, if any.
    pub key: Option<&'static str>,
    /// What AppState executes after closing the palette.
    pub action: Action,
    /// Free-text Ask is kept even when the query also matches another command.
    pub always: bool,
    /// Listed, dimmed, with why in its label; choosing it runs nothing.
    pub unavailable: bool,
}

fn row(
    id: impl Into<String>,
    group: &'static str,
    label: impl Into<String>,
    key: Option<&'static str>,
    action: Action,
) -> Command {
    Command {
        id: id.into(),
        group,
        label: label.into(),
        key,
        action,
        always: false,
        unavailable: false,
    }
}

/// The open session page's catalog, group by group, ahead of the app's own rows.
fn session_rows(catalog: &SessionActions) -> impl Iterator<Item = Command> + '_ {
    catalog.groups.iter().flat_map(|group| {
        group.rows.iter().map(move |found| Command {
            id: format!("session:{}", found.id),
            group: group.title,
            label: match &found.unavailable {
                Some(why) => format!("{} · unavailable: {why}", found.label),
                None => found.label.clone(),
            },
            key: found.key,
            action: Action::Session(found.action.clone()),
            always: false,
            unavailable: found.unavailable.is_some(),
        })
    })
}

/// Commands use the selected canonical feed for records and actions. Only the explicitly
/// selected preview offers mounted Demo illustrations; disconnected has no sample records.
/// `session`: the catalog of the session page on top, listed first.
pub fn commands(
    query: &str,
    socket_configured: bool,
    data: DataState,
    feed: &Feed,
    record: Option<&crate::model::work::Record>,
    form: Form,
    session: Option<&SessionActions>,
) -> Vec<Command> {
    let query = query.trim();
    let live = data != DataState::Disconnected;
    let preview = data == DataState::Fixture;
    let managed = crate::model::WorkspaceMode::of(feed) == crate::model::WorkspaceMode::Managed;
    let ask = preview || (live && feed.capable(corral_protocol::app::ASSISTANT_V1));
    let mut rows: Vec<Command> = session.into_iter().flat_map(session_rows).collect();
    if ask {
        rows.extend(ask_with(query));
    }
    if live && managed {
        live_needs_you(&mut rows, feed, query);
        live_verbs(&mut rows, record, feed);
    }
    rows.extend(
        crate::model::nav::destinations(feed)
            .into_iter()
            .map(|destination| {
                row(
                    format!("go:{}", destination.slug()),
                    "Go to",
                    place_label(destination),
                    destination_key(destination),
                    Action::Go(destination),
                )
            }),
    );
    if live {
        live_open(&mut rows, feed, query);
        let projects = crate::model::projects::known_projects(feed);
        if !projects.is_empty() {
            rows.push(row(
                "scope-all",
                "Scope",
                format!("Scope: {}", crate::model::projects::ALL_PROJECTS),
                None,
                Action::AllProjects,
            ));
        }
        rows.extend(projects.into_iter().map(|id| {
            row(
                format!("project-{id}"),
                "Scope",
                format!("Scope: {id}"),
                None,
                Action::Project(id),
            )
        }));
    }
    rows.push(pair_row(data));
    if live && (preview || feed.capable(corral_protocol::app::BUILD_V1)) {
        rows.push(row(
            "new-build",
            "Sessions",
            "New build session…",
            Some("N"),
            Action::NewBuild,
        ));
    }
    if ask {
        rows.push(row(
            "new-assistant",
            "Sessions",
            "New assistant session",
            None,
            Action::NewAssistant,
        ));
    }
    rows.extend(SettingsSection::ALL.into_iter().map(|section| {
        row(
            format!("settings:{}", section.slug()),
            "Settings",
            format!("Settings: {}", section.label()),
            None,
            Action::Settings(section),
        )
    }));
    if ask {
        rows.push(row("ask", "Ask", "Ask", Some("/"), Action::Ask(None)));
    }
    rows.extend(
        [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark]
            .into_iter()
            .map(|theme| {
                row(
                    format!("theme:{}", theme.slug()),
                    "Appearance",
                    format!("Theme: {}", theme.slug()),
                    None,
                    Action::Theme(theme),
                )
            }),
    );
    rows.extend(
        [
            DataState::Fixture,
            DataState::Daemon,
            DataState::Disconnected,
        ]
        .into_iter()
        .filter(|state| *state != DataState::Daemon || socket_configured)
        .map(|data| {
            row(
                format!("data:{}", data.slug()),
                "Data",
                format!("Data: {}", data.slug()),
                None,
                Action::Data(data),
            )
        }),
    );
    rows.push(row(
        "preview-no-projects",
        "Data",
        "Preview: no projects",
        None,
        Action::PreviewNoProjects,
    ));
    if preview {
        rows.extend(demo_rows().into_iter().filter(|row| match form {
            Form::Phone => matches!(row.action, Action::Share | Action::Lock),
            Form::Desktop | Form::Terminal => matches!(row.action, Action::Hotkey),
        }));
    }
    rows
}

/// The free-text Ask row, unless the text reads as navigation. It is ranked after every
/// command the text also matches (`filtered`), so Enter runs a typed command, not a question.
fn ask_with(query: &str) -> Option<Command> {
    let folded = query.to_lowercase();
    if query.is_empty()
        || ["t-", "go ", "open "]
            .iter()
            .any(|prefix| folded.starts_with(prefix))
    {
        return None;
    }
    let mut ask = row(
        "ask-with",
        "Ask",
        format!("Ask: “{query}”"),
        None,
        Action::Ask(Some(query.into())),
    );
    ask.always = true;
    Some(ask)
}

/// Where pairing happens from here: an enrolled device pairs others from Devices; anything
/// else pairs itself from Connection. The palette only navigates; pairing is that page's.
fn pair_row(data: DataState) -> Command {
    if data == DataState::Daemon {
        row(
            "pair",
            "Device",
            "Pair a new device",
            None,
            Action::Settings(SettingsSection::Devices),
        )
    } else {
        row(
            "pair",
            "Device",
            "Pair this device",
            None,
            Action::Settings(SettingsSection::Connection),
        )
    }
}

/// Adds live rows of one group: those matching the query, a few without one.
fn push_live(
    rows: &mut Vec<Command>,
    group: &'static str,
    query: &str,
    items: impl Iterator<Item = (String, Live)>,
) {
    let folded = query.to_lowercase();
    let limit = if folded.is_empty() {
        LIVE_IDLE
    } else {
        LIVE_MATCHED
    };
    rows.extend(
        items
            .filter(|(label, _)| {
                folded.is_empty() || format!("{group} {label}").to_lowercase().contains(&folded)
            })
            .take(limit)
            .map(|(label, live)| {
                row(
                    format!("live:{}:{}", live.kind().slug(), live.id()),
                    group,
                    label,
                    None,
                    Action::OpenLive(live),
                )
            }),
    );
}

/// Tasks that wait on the owner: Blocked ones, and reviews ready to land.
fn live_needs_you(rows: &mut Vec<Command>, feed: &Feed, query: &str) {
    let open = feed
        .board
        .in_group(crate::model::daemon::Group::NeedsYou)
        .map(|task| {
            (
                format!("{} · {} · {}", task.id, task.title(), task.now()),
                Live::Task(task.id.clone()),
            )
        });
    push_live(rows, "Needs you", query, open);
}

/// The Actions menu of the selected task, or the Session menu of the selected run, then the
/// workspace's own.
fn live_verbs(rows: &mut Vec<Command>, record: Option<&crate::model::work::Record>, feed: &Feed) {
    use crate::model::work::{Press, Record, record_verbs};
    let workspace = Record::Workspace;
    let record = record.unwrap_or(&workspace);
    let verbs = record_verbs(record, feed).into_iter();
    // A verb the record cannot take stays out: the palette has no room to say why.
    rows.extend(verbs.filter(|verb| verb.enabled).map(|verb| {
        let label = match (record, &verb.press) {
            (_, Press::Menu(_)) | (Record::Workspace, _) => verb.label,
            (Record::Task(id) | Record::Session(id), _) => format!("{} {id}", verb.label),
        };
        row(
            format!("verb:{}", verb.slug),
            "Actions",
            label,
            None,
            Action::Work(verb.press),
        )
    }));
    if !matches!(record, Record::Workspace) {
        rows.extend(record_verbs(&workspace, feed).into_iter().map(|verb| {
            row(
                format!("verb:{}", verb.slug),
                "Workspace actions",
                verb.label,
                None,
                Action::Work(verb.press),
            )
        }));
    }
}

/// The task a query names whole (`T-123`, either case of the prefix), spelled as corrald does.
fn task_id(query: &str) -> Option<Id> {
    let number = query
        .strip_prefix("T-")
        .or_else(|| query.strip_prefix("t-"))?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| Id::from(format!("T-{number}")))
}

/// Records the daemon served: board tasks, plans, runs, chats and build sessions. A
/// whole task id the board does not list (a closed task) opens by id, from its detail.
fn live_open(rows: &mut Vec<Command>, feed: &Feed, query: &str) {
    if crate::model::WorkspaceMode::of(feed) == crate::model::WorkspaceMode::Managed {
        if let Some(id) = task_id(query).filter(|id| feed.board.task(id).is_none()) {
            let live = Live::Task(id);
            rows.push(row(
                format!("live:{}:{}", live.kind().slug(), live.id()),
                "Open",
                format!("Open {}", live.id()),
                None,
                Action::OpenLive(live),
            ));
        }
        push_live(
            rows,
            "Open",
            query,
            feed.board.tasks().iter().map(|task| {
                (
                    format!("{} {}", task.id, task.title()),
                    Live::Task(task.id.clone()),
                )
            }),
        );
        push_live(
            rows,
            "Open",
            query,
            feed.plans.iter().map(|(id, plan)| {
                (
                    format!("Plan {id} {}", plan.parent.title),
                    Live::Plan(id.clone()),
                )
            }),
        );
    }
    push_live(
        rows,
        "Open",
        query,
        feed.agents.iter().map(|(id, agent)| {
            (
                format!(
                    "{} {} · {} session",
                    agent.run.task,
                    agent.run.profile,
                    agent.run.role.as_str()
                ),
                Live::Session(id.clone()),
            )
        }),
    );
    push_live(
        rows,
        "Open",
        query,
        feed.assistants.iter().map(|(id, session)| {
            let role = match session.kind {
                corral_protocol::rows::AssistantKind::Build => "build",
                _ => "assistant",
            };
            (
                format!("{id} {} · {role} session", session.title),
                Live::Session(id.clone()),
            )
        }),
    );
}

/// Illustrations offered only where their UI is mounted.
fn demo_rows() -> Vec<Command> {
    vec![
        row(
            "demo-hotkey",
            "Demo",
            "Show hotkey window · demo",
            None,
            Action::Hotkey,
        ),
        row(
            "demo-share",
            "Demo",
            "Show share sheet · demo",
            None,
            Action::Share,
        ),
        row(
            "demo-lock",
            "Demo",
            "Show lock-screen push · demo",
            None,
            Action::Lock,
        ),
    ]
}

/// Commands matching group plus label, case-insensitively. The free-text Ask row always
/// survives, after the commands the text matches, so the first match is the default.
pub fn filtered(
    query: &str,
    socket_configured: bool,
    data: DataState,
    feed: &Feed,
    record: Option<&crate::model::work::Record>,
    form: Form,
    session: Option<&SessionActions>,
) -> Vec<Command> {
    let folded = query.trim().to_lowercase();
    let (ask, mut rows): (Vec<Command>, Vec<Command>) =
        commands(query, socket_configured, data, feed, record, form, session)
            .into_iter()
            .filter(|command| {
                command.always
                    || folded.is_empty()
                    || format!("{} {}", command.group, command.label)
                        .to_lowercase()
                        .contains(&folded)
            })
            .partition(|command| command.always);
    rows.extend(ask);
    rows
}

/// Resolves a selected command without interpreting arbitrary ids as executable commands; an
/// unavailable session row resolves to nothing.
#[allow(clippy::too_many_arguments)]
pub fn run(
    id: &str,
    query: &str,
    socket_configured: bool,
    data: DataState,
    feed: &Feed,
    record: Option<&crate::model::work::Record>,
    form: Form,
    session: Option<&SessionActions>,
) -> Option<Action> {
    commands(query, socket_configured, data, feed, record, form, session)
        .into_iter()
        .find(|command| command.id == id && !command.unavailable)
        .map(|command| command.action)
}

/// The key that opens the page, as a static key cap for renderers: its place's key where the
/// place opens it, Usage's and Artifacts' own. Timeline has none.
pub const fn destination_key(destination: Destination) -> Option<&'static str> {
    match destination {
        Destination::Home => Some("h"),
        Destination::Tasks => Some("t"),
        Destination::Sessions => Some("s"),
        Destination::Feed => Some("a"),
        Destination::Timeline => None,
        Destination::Usage => Some("u"),
        Destination::Artifacts => Some("f"),
        Destination::Settings => Some(","),
    }
}

/// A place's key as a static key cap.
pub const fn area_key(area: crate::model::nav::Area) -> &'static str {
    use crate::model::nav::Area;
    match area {
        Area::Home => "h",
        Area::Tasks => "t",
        Area::Sessions => "s",
        Area::Activity => "a",
        Area::Settings => ",",
    }
}

/// A page as the palette names it: Activity's pages under their place.
fn place_label(destination: Destination) -> String {
    use crate::model::nav::Area;
    match destination.area() {
        Area::Activity => format!("{} › {}", Area::Activity.label(), destination.label()),
        _ => destination.label().to_owned(),
    }
}

/// Looks up a row's shortcut without allocating the command table during row rendering.
pub fn shortcut(id: &str) -> Option<&'static str> {
    match id {
        "ask" => Some("/"),
        "new-build" => Some("N"),
        _ => id
            .strip_prefix("go:")
            .and_then(Destination::from_slug)
            .and_then(destination_key),
    }
}
