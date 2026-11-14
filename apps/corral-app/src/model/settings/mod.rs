//! Settings: the fixture preview's illustrative sections, pairing walkthrough and actions, and
//! the daemon session's live sections (`live`), configured profiles (`profiles`), owner edits
//! (`edits`) and autopilot grant (`autopilot`), with renderer-ready views.

pub mod autopilot;
mod data;
mod edits;
pub mod live;
mod pairing;
pub mod profiles;

pub use edits::{ADD_PROFILE, Edits, Pending, remove_slot};
pub use live::daemon_connection_details;
pub use pairing::*;

use crate::model::board::Mark;
use crate::model::effect::{Conn, Effect, Env, Risk};
use crate::model::hue::Hue;
use crate::model::nav::SettingsSection;
use crate::model::text::{Rich, span};

pub use data::{DEVICES, NOTIFICATIONS, PAYLOAD, PROFILES, QUEUE_RULES, STEPS};

/// A paired device and its permissions in the illustrative snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub name: &'static str,
    pub platform: &'static str,
    pub seen: &'static str,
    pub reach: &'static str,
    pub this_device: bool,
    pub may_instruct: &'static str,
}
/// A pairing instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub title: &'static str,
    pub detail: &'static str,
}
/// A notification switch identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum NotificationKind {
    NeedsYou,
    Plan,
    Stuck,
    Escalated,
    Daemon,
    Assistant,
    Landed,
}
/// A notification label, description and initial switch value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notification {
    pub kind: NotificationKind,
    pub label: &'static str,
    pub detail: &'static str,
    pub default_on: bool,
}
/// One offline action and its queue policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueRule {
    pub action: &'static str,
    pub queued: bool,
    pub reason: &'static str,
}
impl QueueRule {
    /// Status chip for the offline action.
    pub fn chip(&self) -> (&'static str, Hue) {
        if self.queued {
            ("queued", Hue::Held)
        } else {
            ("refused", Hue::Fail)
        }
    }
}
/// A profile in the assistant's model selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    pub model: &'static str,
    pub note: &'static str,
}
/// Phone pairing sheet currently shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sheet {
    Pair,
    Scan,
}
/// Label of the phone sheet's close button.
pub const SHEET_DONE: &str = "Done";
/// Local settings interaction state. Navigation owns the selected section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub pair: u8,
    pub sheet: Option<Sheet>,
    pub notifications: [bool; 7],
    /// The daemon session's pending owner edits and confirmations.
    pub edits: Edits,
}
impl Default for State {
    fn default() -> Self {
        Self {
            pair: 2,
            sheet: None,
            notifications: [true, true, true, true, true, false, false],
            edits: Edits::default(),
        }
    }
}
impl State {
    /// Move the pairing step, clamped to the four instructions and completion.
    pub fn pair_step(&mut self, delta: i32) -> Vec<Effect> {
        self.pair = (i32::from(self.pair).saturating_add(delta)).clamp(0, 4) as u8;
        vec![]
    }
    /// Confirm the matching code and show the pairing result.
    pub fn pair_confirm(&mut self) -> Vec<Effect> {
        self.pair = 4;
        vec![Effect::Toast(
            "Paired pawis-ipad · actions will carry via=app:pawis-ipad".into(),
        )]
    }
    /// Reject pairing and burn the token.
    pub fn pair_reject(&mut self) -> Vec<Effect> {
        self.pair = 0;
        vec![Effect::Toast("Pairing rejected · token burned".into())]
    }
    /// Ask for confirmation before revoking a device.
    pub fn revoke(&mut self, name: &str) -> Vec<Effect> {
        vec![Effect::Confirm(Risk::Revoke, Some(name.to_owned()))]
    }
    /// Open either phone pairing sheet.
    pub fn open_sheet(&mut self, sheet: Sheet) -> Vec<Effect> {
        self.sheet = Some(sheet);
        vec![]
    }
    /// Close the phone pairing sheet.
    pub fn close_sheet(&mut self) -> Vec<Effect> {
        self.sheet = None;
        vec![]
    }
    /// Toggle a notification category.
    pub fn toggle_notification(&mut self, kind: NotificationKind) -> Vec<Effect> {
        let on = &mut self.notifications[kind as usize];
        *on = !*on;
        vec![]
    }
    /// Select a connection state card.
    pub fn set_connection(&mut self, conn: Conn) -> Vec<Effect> {
        vec![Effect::SetConn(conn)]
    }
    /// Open a section through the app's navigation state.
    pub fn set_section(&mut self, section: SettingsSection) -> Vec<Effect> {
        vec![Effect::GoSettings(section)]
    }
    /// Produce the mockup response for a device's Turn on button.
    pub fn turn_on(&mut self, name: &str) -> Vec<Effect> {
        vec![Effect::Toast(format!(
            "Turning instructing on for {name} asks for a confirmation on this device"
        ))]
    }
    /// Produce the mockup response for discarding a queued item.
    pub fn discard_queue(&mut self) -> Vec<Effect> {
        vec![Effect::Toast("Removed from the local queue".into())]
    }
    /// Produce the mockup response for removing a selector profile.
    pub fn remove_profile(&mut self, name: &str) -> Vec<Effect> {
        vec![Effect::Toast(format!(
            "Removed {name} from the selector · the profile itself is unchanged"
        ))]
    }
    /// Produce the mockup response for adding a selector profile.
    pub fn add_profile(&mut self) -> Vec<Effect> {
        vec![Effect::Toast(
            "Add a profile: any corral-pi profile that names a model".into(),
        )]
    }
}
/// Action a settings control invokes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    PairStep(i32),
    PairConfirm,
    PairReject,
    Revoke(&'static str),
    OpenSheet(Sheet),
    CloseSheet,
    ToggleNotification(NotificationKind),
    SetConnection(Conn),
    SetSection(SettingsSection),
    TurnOn(&'static str),
    DiscardQueue,
    RemoveProfile(&'static str),
    AddProfile,
}
impl State {
    /// Dispatch a renderer's action.
    pub fn act(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::PairStep(d) => self.pair_step(d),
            Action::PairConfirm => self.pair_confirm(),
            Action::PairReject => self.pair_reject(),
            Action::Revoke(n) => self.revoke(n),
            Action::OpenSheet(s) => self.open_sheet(s),
            Action::CloseSheet => self.close_sheet(),
            Action::ToggleNotification(k) => self.toggle_notification(k),
            Action::SetConnection(c) => self.set_connection(c),
            Action::SetSection(s) => self.set_section(s),
            Action::TurnOn(n) => self.turn_on(n),
            Action::DiscardQueue => self.discard_queue(),
            Action::RemoveProfile(n) => self.remove_profile(n),
            Action::AddProfile => self.add_profile(),
        }
    }
}
/// Emphasis of a settings action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    Primary,
    Ghost,
    Quiet,
    Danger,
}

/// A labelled control with its action and emphasis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: &'static str,
    pub action: Action,
    pub emphasis: Emphasis,
    pub enabled: bool,
}
fn button(label: &'static str, action: Action, emphasis: Emphasis) -> Button {
    Button {
        label,
        action,
        emphasis,
        enabled: true,
    }
}
/// A connection state card and its selected state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionCard {
    pub conn: Conn,
    pub title: &'static str,
    pub detail: &'static str,
    pub hue: Hue,
    pub pressed: bool,
    pub action: Action,
}
/// One local queue entry, with discard available only for queued entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalQueueRow {
    pub mark: Mark,
    pub text: Rich,
    pub detail: &'static str,
    pub hue: Hue,
    pub discard: Option<Button>,
}
/// The connection inspector, cards, local queue and offline policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionView {
    pub title: &'static str,
    pub phone_title: &'static str,
    pub state_heading: &'static str,
    pub cards: Vec<ConnectionCard>,
    pub now_heading: &'static str,
    pub details: &'static [(&'static str, &'static str)],
    pub illustrative: &'static str,
    pub local_heading: Option<&'static str>,
    pub local_queue: Vec<LocalQueueRow>,
    pub rules_heading: &'static str,
    pub rules: &'static [QueueRule],
}
/// Build the connection state inspector and policy table.
pub fn connection(_state: &State, env: Env) -> ConnectionView {
    let cards = [
        (
            Conn::Healthy,
            "corrald healthy",
            "direct · loopback",
            Hue::Ok,
        ),
        (
            Conn::Degraded,
            "degraded · sync stalled 4m",
            "PM relaunching",
            Hue::Held,
        ),
        (
            Conn::Down,
            "corrald down",
            "herdr-core unreachable · PM absent",
            Hue::Fail,
        ),
        (
            Conn::Relay,
            "relay · e2e",
            "relay sees sizes, not content",
            Hue::Ok,
        ),
        (
            Conn::Offline,
            "offline · last sync 22:08",
            "1 queued · rest refused",
            Hue::Held,
        ),
    ]
    .into_iter()
    .map(|(conn, title, detail, hue)| ConnectionCard {
        conn,
        title,
        detail,
        hue,
        pressed: env.conn == conn,
        action: Action::SetConnection(conn),
    })
    .collect();
    let local_queue = if env.conn.refuses() {
        vec![
            LocalQueueRow {
                mark: Mark::Stale,
                text: vec![
                    span("Answer "),
                    span("T-868/R1/Q1").mono(),
                    span(" · option 1"),
                ],
                detail: "queued 22:10 · expects rev 2",
                hue: Hue::Held,
                discard: Some(button("Discard", Action::DiscardQueue, Emphasis::Quiet)),
            },
            LocalQueueRow {
                mark: Mark::Stale,
                text: vec![span("Message to the quick thread")],
                detail: "queued on this device 22:11 · delivered once",
                hue: Hue::Held,
                discard: Some(button("Discard", Action::DiscardQueue, Emphasis::Quiet)),
            },
            LocalQueueRow {
                mark: Mark::Fail,
                text: vec![span("Accept plan T-885 P2")],
                detail: "refused 22:11 · needs live connection",
                hue: Hue::Fail,
                discard: None,
            },
        ]
    } else {
        vec![]
    };
    ConnectionView {
        title: "Connection and health",
        phone_title: "Connection",
        state_heading: "State · try each",
        cards,
        now_heading: "Now",
        details: data::details(env.conn),
        illustrative: "paths, revisions and addresses illustrative",
        local_heading: env.conn.refuses().then_some("On this device"),
        local_queue,
        rules_heading: "When corrald can't be reached",
        rules: &QUEUE_RULES,
    }
}
/// A notification row with its current switch and action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationView {
    pub notification: Notification,
    pub on: bool,
    pub label: &'static str,
    pub hue: Hue,
    pub action: Action,
}
/// Notification section and push explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationsView {
    pub title: &'static str,
    pub rows: Vec<NotificationView>,
    pub note: Rich,
}
/// Build the current notification switches.
pub fn notifications(state: &State, _env: Env) -> NotificationsView {
    NotificationsView {
        title: "Notifications",
        rows: NOTIFICATIONS
            .iter()
            .map(|n| {
                let on = state.notifications[n.kind as usize];
                NotificationView {
                    notification: *n,
                    on,
                    label: if on { "on" } else { "off" },
                    hue: if on { Hue::Ok } else { Hue::Neutral },
                    action: Action::ToggleNotification(n.kind),
                }
            })
            .collect(),
        note: vec![
            span(
                "Push is a content-free wake through the relay; the phone fetches the text end-to-end. Global hotkey for the Ask window: ",
            ),
            span("Ctrl Alt Space").mono(),
            span(" ("),
            span("⌥ Space").mono(),
            span(" on macOS), configurable."),
        ],
    }
}
/// One machine in the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    pub name: &'static str,
    pub illustrative: &'static str,
    pub role: &'static str,
    pub runtime: &'static str,
    pub agents: &'static str,
    pub reach: &'static str,
}
/// The machine table fixture.
pub const MACHINES: [Machine; 1] = [Machine {
    name: "workstation",
    illustrative: "name illustrative",
    role: "daemon host",
    runtime: "herdr-core 0.1 · Herdr TUI (legacy)",
    agents: "6 + 1",
    reach: "local",
}];
/// Machines table and explanatory notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachinesView {
    pub title: &'static str,
    pub columns: [&'static str; 5],
    pub machines: &'static [Machine],
    pub notice: Rich,
}
/// Build the Machines section.
pub fn machines(_state: &State, _env: Env) -> MachinesView {
    MachinesView {
        title: "Machines",
        columns: ["Machine", "Role", "Runtime", "Agents", "Reach"],
        machines: &MACHINES,
        notice: vec![
            span("More machines come later.").bold(),
            span(
                " herdr-core machines join over SSH; agents then show their machine on the Agents wall and in every session header. Nothing here yet needs a decision.",
            ),
        ],
    }
}
/// Selector profile row; the default profile has a chip instead of a remove button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileView {
    pub profile: Profile,
    /// Default profile note and its state colour.
    pub chip: Option<(&'static str, Hue)>,
    pub remove: Option<Button>,
}
/// The configured rollover-summary profile and fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryProfile {
    pub label: &'static str,
    pub profile: &'static str,
    pub model: &'static str,
    pub detail: &'static str,
    pub fallback_label: &'static str,
    pub fallback: &'static str,
    pub fallback_detail: &'static str,
}
/// Summary profile fixture.
pub const SUMMARY_PROFILE: SummaryProfile = SummaryProfile {
    label: "Summary written by",
    profile: "pi-luna",
    model: "gpt-6-luna · low",
    detail: "one call per roll; reads the whole conversation at that model's price",
    fallback_label: "If that profile cannot run",
    fallback: "the thread's current model",
    fallback_detail: "host down: the last part of the transcript is carried instead",
};
/// Assistant model selector settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantView {
    pub title: &'static str,
    pub intro: &'static str,
    pub models_heading: &'static str,
    pub models_key: &'static str,
    pub profiles: Vec<ProfileView>,
    pub add: Button,
    pub summary_heading: &'static str,
    pub summary_key: &'static str,
    pub summary: SummaryProfile,
    pub illustrative: &'static str,
}
/// Build the Assistant settings section.
pub fn assistant(_state: &State, _env: Env) -> AssistantView {
    AssistantView {
        title: "Assistant",
        intro: "The quick thread answers with the assistant profile. The composer's model selector offers the profiles listed here; a switch applies to the next message and keeps the conversation.",
        models_heading: "Models in the selector",
        models_key: "[assistant.quick] models",
        profiles: PROFILES
            .iter()
            .map(|p| ProfileView {
                profile: *p,
                chip: (!p.note.is_empty()).then_some((p.note, Hue::Ok)),
                remove: p
                    .note
                    .is_empty()
                    .then(|| button("Remove", Action::RemoveProfile(p.name), Emphasis::Quiet)),
            })
            .collect(),
        add: button("Add a profile", Action::AddProfile, Emphasis::Ghost),
        summary_heading: "When the quick thread rolls over",
        summary_key: "[assistant.quick] summary_agent",
        summary: SUMMARY_PROFILE,
        illustrative: "profile names illustrative; model ids from corral's price catalog",
    }
}
/// Owner-only settings explanation shared by Autopilot and General.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerView {
    pub title: &'static str,
    pub description: Rich,
}
fn owner_view(title: &'static str) -> OwnerView {
    OwnerView {
        title,
        description: vec![
            span(
                "Owner-only settings keep their current authority rules; changing them from a device needs a live connection and is recorded with ",
            ),
            span("via=app:<device>").mono(),
            span("."),
        ],
    }
}
/// Build Autopilot and grants settings.
pub fn autopilot(_state: &State, _env: Env) -> OwnerView {
    owner_view("Autopilot & grants")
}
/// Build General settings.
pub fn general(_state: &State, _env: Env) -> OwnerView {
    owner_view("General")
}
/// One phone Settings root navigation row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootRow {
    pub section: SettingsSection,
    pub label: &'static str,
    pub subtitle: &'static str,
    pub action: Action,
}
/// The phone Settings root list: every section the other layouts list in their sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootView {
    pub title: &'static str,
    pub back_label: &'static str,
    pub rows: Vec<RootRow>,
}
/// Build the phone root list using the current connection's phone reach text.
pub fn phone_root(_state: &State, env: Env) -> RootView {
    let reach = match env.conn {
        Conn::Healthy | Conn::Degraded | Conn::Down => "direct · tailscale",
        Conn::Relay => "relay · e2e",
        Conn::Offline => "offline · last sync 22:08",
    };
    let rows = [
        (
            SettingsSection::Devices,
            "4 paired · this phone via relay · e2e",
        ),
        (SettingsSection::Machines, "workstation · daemon host"),
        (SettingsSection::Connection, reach),
        (SettingsSection::Notifications, "5 kinds on"),
        (
            SettingsSection::Assistant,
            "4 models in the selector · summaries by pi-luna",
        ),
        (
            SettingsSection::Autopilot,
            "owner only · needs live connection",
        ),
        (
            SettingsSection::General,
            "owner only · needs live connection",
        ),
    ]
    .into_iter()
    .map(|(section, subtitle)| RootRow {
        section,
        label: section.label(),
        subtitle,
        action: Action::SetSection(section),
    })
    .collect();
    RootView {
        title: "Settings",
        back_label: "More",
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pairing_steps_do_not_escape_bounds() {
        let mut state = State::default();
        state.pair_step(i32::MIN);
        assert_eq!(state.pair, 0);
        state.pair_step(i32::MAX);
        assert_eq!(state.pair, 4);
    }
    #[test]
    fn qr_finders_are_in_three_corners_without_transposing() {
        let cells = qr();
        for (ox, oy) in [(0, 0), (22, 0), (0, 22)] {
            for y in 0..7 {
                for x in 0..7 {
                    assert_eq!(
                        cells[oy + y][ox + x],
                        x == 0
                            || x == 6
                            || y == 0
                            || y == 6
                            || (x >= 2 && x <= 4 && y >= 2 && y <= 4)
                    );
                }
            }
        }
        assert!(!cells[7][0]);
        assert!(!cells[0][7]);
    }
}
