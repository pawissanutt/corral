//! Ask is one chat for questions, ideas and instructions. The app owns the draft and target.
//! Render `composer`, `drawer`, `thread`, `agents`, `thread_header`, `hotkey_window` and
//! `share_sheet`; dispatch their typed actions with `State::act`. Timers call `State::tick`.
//! `LABELS` and `FILED_LABELS` supply captions for the structured records, including field labels.
//! All fixtures are illustrative except the linked board facts and protocol vocabulary.

mod cards;
mod data;
mod labels;
mod surfaces;
mod thread;
mod views;

pub use cards::*;
pub use data::*;
pub use labels::*;
pub use surfaces::*;
pub use thread::*;
pub use views::*;

use crate::model::effect::{Conn, Effect, Env, Page, Risk};
use crate::model::nav::{AskTarget, Form};

/// The Assistant studio switch, not a composer mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Answer is streaming or complete, according to `State::stream`.
    #[default]
    Answering,
    /// Host is starting after first use, restart or host exit.
    Starting,
    /// The last turn was stopped; the host is still live.
    Stopped,
    /// Message waits for its host.
    Down,
    /// Message waits for machine capacity.
    Load,
    /// Message waits for provider quota.
    Quota,
}

impl Mode {
    /// Studio options in source order.
    pub const ALL: [Mode; 6] = [
        Self::Answering,
        Self::Starting,
        Self::Stopped,
        Self::Down,
        Self::Load,
        Self::Quota,
    ];

    /// Verbatim studio option label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Answering => "answering",
            Self::Starting => "starting (first use or restart)",
            Self::Stopped => "stopped · you stopped the turn",
            Self::Down => "waiting · host down",
            Self::Load => "waiting · machine load",
            Self::Quota => "waiting · provider quota",
        }
    }

    /// Deep-link value of the studio option.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Answering => "answering",
            Self::Starting => "starting",
            Self::Stopped => "stopped",
            Self::Down => "down",
            Self::Load => "load",
            Self::Quota => "quota",
        }
    }

    /// Parses the mockup's `asst=` values.
    pub fn from_slug(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.slug() == value)
    }

    /// Why a message can be stored but the assistant cannot run.
    pub const fn wait_reason(self) -> Option<WaitReason> {
        match self {
            Self::Down => Some(WaitReason::HostDown),
            Self::Load => Some(WaitReason::MachineLoad),
            Self::Quota => Some(WaitReason::ProviderQuota),
            _ => None,
        }
    }
}

/// A waiting message's non-expiring blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// Provider host exited.
    HostDown,
    /// Memory is below the machine-load minimum.
    MachineLoad,
    /// Subscription window exhausted.
    ProviderQuota,
}

impl WaitReason {
    /// Short reason on messages and chips.
    pub const fn label(self) -> &'static str {
        match self {
            Self::HostDown => "host down",
            Self::MachineLoad => "machine load",
            Self::ProviderQuota => "provider quota",
        }
    }

    /// Full explanation in the waiting block.
    pub const fn explanation(self) -> &'static str {
        match self {
            Self::HostDown => "A-7's host stopped (provider exited).",
            Self::MachineLoad => {
                "The machine-load probe reports memory below its minimum (illustrative: 1.1 GB free, 2 GB needed)."
            }
            Self::ProviderQuota => {
                "The OpenAI subscription window is exhausted until 23:58 (illustrative)."
            }
        }
    }
}

/// The three no-model waiting reasons in studio order.
pub const WAIT_REASONS: [WaitReason; 3] = [
    WaitReason::HostDown,
    WaitReason::MachineLoad,
    WaitReason::ProviderQuota,
];

/// Small global-hotkey window content states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HotkeyState {
    /// Input only.
    Typing,
    /// Answer and filed card.
    #[default]
    Answer,
    /// Filed card only.
    Filed,
}

/// Share extension steps; saving only writes an outbox item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareStep {
    /// System app chooser.
    Sheet,
    /// Optional caption and Send.
    Compose,
    /// Locally saved, not yet delivered.
    Saved,
    /// Delivered and filed in the quick thread.
    Delivered,
}

/// A typed message's delivery or terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageState {
    /// Already delivered owner input.
    Sent,
    /// On-device durable queue, never a refusal.
    QueuedOnDevice,
    /// Stored in the shared thread, waiting for a model.
    Waiting(WaitReason),
    /// Arrived over fifteen minutes after it was typed.
    Late,
    /// Delivered to the PM inbox.
    Delivered,
    /// Waiting until the PM next takes input.
    QueuedForPm,
    /// PM reply attached to the same item.
    PmReplied,
    /// Filed by the owner with no model.
    Saved(&'static str),
    /// Owner withdrew it without filing or acting.
    Withdrawn,
}

impl MessageState {
    /// Visible owner-message tag.
    pub fn label(self) -> String {
        match self {
            Self::Sent => "sent".into(),
            Self::QueuedOnDevice => "queued on this device".into(),
            Self::Waiting(reason) => format!("waiting for assistant · {}", reason.label()),
            Self::Late => "late · cannot instruct".into(),
            Self::Delivered => "delivered".into(),
            Self::QueuedForPm => "queued for PM".into(),
            Self::PmReplied => "PM replied".into(),
            Self::Saved(id) => format!("saved · {id}"),
            Self::Withdrawn => "withdrawn".into(),
        }
    }

    /// Late owner input is still answerable and fileable, but never authoritative for an action.
    pub const fn can_instruct(self) -> bool {
        matches!(self, Self::Sent)
    }

    /// Whether File as idea now and Withdraw may settle this message.
    pub const fn pending(self) -> bool {
        matches!(self, Self::QueuedOnDevice | Self::Waiting(_))
    }

    /// Semantic colour of the normal owner-message tag.
    pub const fn hue(self) -> crate::model::hue::Hue {
        use crate::model::hue::Hue;
        match self {
            Self::QueuedOnDevice | Self::Waiting(_) | Self::Late => Hue::Held,
            _ => Hue::Neutral,
        }
    }

    /// Semantic colour of the PM item's delivery state.
    pub const fn pm_hue(self) -> crate::model::hue::Hue {
        use crate::model::hue::Hue;
        match self {
            Self::PmReplied => Hue::Ok,
            Self::Delivered => Hue::Neutral,
            _ => Hue::Held,
        }
    }
}

/// A message typed during this fixture session; no assistant reply is fabricated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMessage {
    /// Unchanged submitted text, trimmed as in the mockup.
    pub text: String,
    /// Originating device.
    pub device: &'static str,
    /// PM inbox delivery rather than assistant input.
    pub pm: bool,
    /// Delivery state displayed immediately.
    pub state: MessageState,
    /// Session to which it belongs.
    pub thread: &'static str,
}

/// Assistant domain state; the app separately owns Ask's target, draft and visibility.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// Removable bounded-context attachment.
    pub chip: Option<&'static str>,
    /// Selected profile, absent for the default.
    pub model: Option<&'static str>,
    /// Model list visibility.
    pub model_menu: bool,
    /// Host/turn studio state.
    pub mode: Mode,
    /// Stream position from zero to one; no timer lives here.
    pub stream: f32,
    /// Optional replacement for the fixture's last question.
    pub last_q: Option<String>,
    /// Global hotkey content state.
    pub hotkey: HotkeyState,
    /// Delivery state of the last nonblank submission from the hotkey window.
    pub hotkey_sent: Option<MessageState>,
    /// Current phone share extension step, absent when closed.
    pub share: Option<ShareStep>,
    /// Messages submitted during this fixture session.
    pub sent: Vec<SentMessage>,
    /// Filed card whose assistant-written fields are editable.
    pub edit: Option<&'static str>,
    /// Last fixture question was filed as written.
    pub saved_now: bool,
    /// Last fixture question was withdrawn.
    pub withdrawn: bool,
    /// Open thread, initially A-7.
    pub thread: &'static str,
}

impl Default for State {
    fn default() -> Self {
        Self {
            chip: None,
            model: None,
            model_menu: false,
            mode: Mode::Answering,
            stream: 1.0,
            last_q: None,
            hotkey: HotkeyState::Answer,
            hotkey_sent: None,
            share: None,
            sent: Vec::new(),
            edit: None,
            saved_now: false,
            withdrawn: false,
            thread: "A-7",
        }
    }
}

impl State {
    /// Selects the session opened by the app's `Page::Thread` navigation.
    pub fn open_thread(&mut self, id: &'static str) {
        self.thread = id;
        self.model_menu = false;
    }

    /// Attaches or removes the context supplied by Ask about this.
    pub fn attach(&mut self, chip: Option<&'static str>) {
        self.chip = chip;
    }

    /// Escape closes the model list first; returns whether it consumed Escape.
    pub fn close_menu(&mut self) -> bool {
        let open = self.model_menu;
        self.model_menu = false;
        open
    }

    /// Current pill name, using the assistant profile by default.
    pub fn model_name(&self) -> &'static str {
        self.model.unwrap_or(MODELS[0].name)
    }

    /// Selects the next configured profile, wrapping to the default.
    pub fn cycle_model(&mut self) -> Vec<Effect> {
        let next = MODELS
            .iter()
            .position(|m| m.name == self.model_name())
            .map_or(0, |i| (i + 1) % MODELS.len());
        self.pick_model(MODELS[next].name)
    }

    /// Picks a profile without resetting conversation or stream history.
    pub fn pick_model(&mut self, name: &'static str) -> Vec<Effect> {
        if !MODELS.iter().any(|m| m.name == name) {
            return Vec::new();
        }
        self.model = Some(name);
        self.model_menu = false;
        vec![Effect::Toast(format!(
            "Model: {name} · applies to your next message · conversation kept"
        ))]
    }

    /// Changes studio mode; resuming starts a fresh stream at 0.1 as in the mockup.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.saved_now = false;
        self.withdrawn = false;
        if mode == Mode::Answering {
            self.stream = 0.1;
        }
    }

    /// Reveals one 0.12 stream step; true only if another tick is needed.
    pub fn tick(&mut self) -> bool {
        if self.mode != Mode::Answering || self.stream >= 1.0 {
            return false;
        }
        self.stream = (self.stream + 0.12).min(1.0);
        self.stream < 1.0
    }

    /// Stores quick-thread input immediately, even if a named page is behind the Ask drawer.
    /// Offline and down queue, never refuse. The app clears its draft after a nonempty send.
    pub fn send(&mut self, text: &str, target: AskTarget, env: Env) -> Vec<Effect> {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let pm = target == AskTarget::Pm;
        let state = if env.conn.refuses() {
            MessageState::QueuedOnDevice
        } else if pm {
            if env.conn == Conn::Degraded {
                MessageState::QueuedForPm
            } else {
                MessageState::Delivered
            }
        } else if let Some(reason) = self.mode.wait_reason() {
            MessageState::Waiting(reason)
        } else {
            MessageState::Sent
        };
        self.sent.push(SentMessage {
            text: text.into(),
            device: env.device(),
            pm,
            state,
            thread: "A-7",
        });
        if env.form == Form::Phone {
            vec![Effect::CloseAsk, Effect::Open(Page::Thread("A-7"))]
        } else {
            Vec::new()
        }
    }

    /// Sends to the assistant quick thread and retains its actual state beneath the hotkey field.
    /// Blank input leaves both the result and the previous submission state unchanged.
    pub fn send_from_hotkey(&mut self, text: &str, env: Env) -> Vec<Effect> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        let effects = self.send(text, AskTarget::Assistant, env);
        self.hotkey = HotkeyState::Typing;
        self.hotkey_sent = self.sent.last().map(|message| message.state);
        effects
    }

    /// A named session's own composer submits explicitly to its id, not Ask's shared target.
    /// Online replies are not simulated in the source; offline input still queues on the device.
    pub fn send_to_thread(&mut self, id: &'static str, text: &str, env: Env) -> Vec<Effect> {
        if id == "A-7" {
            return self.send(text, AskTarget::Assistant, env);
        }
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        if env.conn.refuses() {
            self.sent.push(SentMessage {
                text: text.into(),
                device: env.device(),
                pm: false,
                state: MessageState::QueuedOnDevice,
                thread: id,
            });
            return Vec::new();
        }
        vec![Effect::Toast(format!(
            "Sent to {id} · its reply is not simulated in the mock"
        ))]
    }

    /// Files every queued/waiting message as written, without invoking a model.
    pub fn file_now(&mut self) -> Vec<Effect> {
        self.saved_now = true;
        for message in &mut self.sent {
            if message.state.pending() {
                message.state = MessageState::Saved("T-904");
            }
        }
        vec![Effect::Toast(
            "Filed by you, as written · no model ran".into(),
        )]
    }

    /// Withdraws queued/waiting messages; their text remains readable.
    pub fn withdraw(&mut self) -> Vec<Effect> {
        self.withdrawn = true;
        for message in &mut self.sent {
            if message.state.pending() {
                message.state = MessageState::Withdrawn;
            }
        }
        vec![Effect::Toast("Withdrawn · nothing filed or run".into())]
    }

    /// Enters or leaves a filed card's edit state; Save and Cancel both leave it in the mockup.
    pub fn edit(&mut self, id: Option<&'static str>) {
        self.edit = id;
    }

    /// Stops only the current turn; its partial answer and live host remain.
    pub fn stop(&mut self) {
        self.set_mode(Mode::Stopped);
    }

    /// Opens the quick thread in Agents, closing Ask.
    pub fn open_in_agents(&mut self) -> Vec<Effect> {
        self.open_thread("A-7");
        vec![Effect::CloseAsk, Effect::Open(Page::Thread("A-7"))]
    }

    /// Opens Ask, preserving an existing context chip unless a replacement is supplied.
    pub fn open_ask(&mut self, chip: Option<&'static str>) -> Vec<Effect> {
        if chip.is_some() {
            self.chip = chip;
        }
        vec![Effect::Ask {
            chip: self.chip,
            text: None,
        }]
    }

    /// Closes Ask and clears its transient chip and model list.
    pub fn close_ask(&mut self) -> Vec<Effect> {
        self.chip = None;
        self.model_menu = false;
        vec![Effect::CloseAsk]
    }

    /// Resolves Ask's keys without owning the app's target or draft.
    /// The app applies `target` and clears its draft only when `clear_draft` is true.
    pub fn key(
        &mut self,
        key: Key,
        text: &str,
        target: AskTarget,
        named_session: Option<&'static str>,
        env: Env,
    ) -> KeyOutcome {
        match key {
            Key::Escape => KeyOutcome {
                effects: if self.close_menu() {
                    Vec::new()
                } else {
                    self.close_ask()
                },
                target: None,
                clear_draft: false,
            },
            Key::CycleModel => KeyOutcome {
                effects: self.cycle_model(),
                target: None,
                clear_draft: false,
            },
            Key::ToggleTarget => KeyOutcome {
                effects: Vec::new(),
                target: Some(if target == AskTarget::Pm {
                    AskTarget::Assistant
                } else {
                    AskTarget::Pm
                }),
                clear_draft: false,
            },
            Key::Send => KeyOutcome {
                effects: if let Some(id) = named_session {
                    self.send_to_thread(id, text, env)
                } else {
                    self.send(text, target, env)
                },
                target: None,
                clear_draft: !text.trim().is_empty(),
            },
        }
    }

    /// Dispatches descriptors from any builder. Target switching and draft mutation belong to the app.
    pub fn act(&mut self, action: Action, text: &str, target: AskTarget, env: Env) -> Vec<Effect> {
        match action {
            Action::OpenAsk(chip) => self.open_ask(chip),
            Action::ToggleModelMenu => {
                self.model_menu = !self.model_menu;
                Vec::new()
            }
            Action::PickModel(name) => self.pick_model(name),
            Action::Unchip => {
                self.attach(None);
                Vec::new()
            }
            Action::CloseAsk => self.close_ask(),
            Action::OpenQuickThread | Action::HotkeyOpen => self.open_in_agents(),
            Action::OpenThread(id) => {
                self.open_thread(id);
                vec![Effect::CloseAsk, Effect::Open(Page::Thread(id))]
            }
            Action::Send => self.send(text, target, env),
            Action::SendToThread(id) => self.send_to_thread(id, text, env),
            Action::SetMode(mode) => {
                self.set_mode(mode);
                Vec::new()
            }
            Action::FileNow => self.file_now(),
            Action::Withdraw => self.withdraw(),
            Action::Edit(id) => {
                self.edit(id);
                Vec::new()
            }
            Action::Share(step) => {
                self.share = step;
                Vec::new()
            }
            Action::Hotkey(state) => {
                self.hotkey = state;
                Vec::new()
            }
            Action::Toast(text) => vec![Effect::Toast(text.into())],
            Action::Merge(id) => vec![Effect::Toast(format!(
                "Merge {id} into: type a task id (T-852, T-874…) · your words move as a comment"
            ))],
            Action::Confirm(risk, name) => vec![Effect::Confirm(risk, name.map(str::to_owned))],
            Action::Open(page) => vec![Effect::Open(page)],
            Action::Grant(grant) => vec![Effect::Toast(grant.outcome(env).into())],
        }
    }

    /// Applies a decoded domain deep-link token. Ask visibility, target and draft stay app-owned.
    pub fn apply_link(&mut self, token: &str, env: Env) -> bool {
        if token == "modelmenu" {
            self.model_menu = true;
            return true;
        }
        if token == "savednow" {
            self.saved_now = true;
            return true;
        }
        if token == "withdrawn" {
            self.withdrawn = true;
            return true;
        }
        if token == "share" {
            self.share = Some(ShareStep::Sheet);
            return true;
        }
        let Some((key, value)) = token.split_once('=') else {
            return false;
        };
        match key {
            "model" => {
                let Some(model) = MODELS.iter().find(|m| m.name == value) else {
                    return false;
                };
                self.model = Some(model.name);
            }
            "asst" => {
                let Some(mode) = Mode::from_slug(value) else {
                    return false;
                };
                self.mode = mode;
            }
            "stream" => {
                let Ok(position) = value.parse::<f32>() else {
                    return false;
                };
                if !position.is_finite() {
                    return false;
                }
                self.stream = position.clamp(0.0, 1.0);
            }
            "edit" => {
                self.edit = filed(value).map(|f| f.id);
            }
            "athread" => {
                let Some(session) = session(value) else {
                    return false;
                };
                self.thread = session.id;
            }
            "hk" => {
                self.hotkey = match value {
                    "typing" => HotkeyState::Typing,
                    "answer" => HotkeyState::Answer,
                    "filed" => HotkeyState::Filed,
                    _ => return false,
                };
            }
            "share" => {
                self.share = match value {
                    "sheet" => Some(ShareStep::Sheet),
                    "compose" => Some(ShareStep::Compose),
                    "saved" => Some(ShareStep::Saved),
                    "delivered" => Some(ShareStep::Delivered),
                    "" => None,
                    _ => return false,
                };
            }
            "sent" => {
                let state = match value {
                    "queued" | "queuedpm" => MessageState::QueuedOnDevice,
                    "waiting" => MessageState::Waiting(WaitReason::HostDown),
                    "sent" => MessageState::Sent,
                    _ => return false,
                };
                let pm = value == "queuedpm";
                self.sent = vec![SentMessage {
                    text: if pm {
                        "hold T-894 if its gate fails twice"
                    } else {
                        "remind me to look at T-852's op log first thing tomorrow"
                    }
                    .into(),
                    device: env.device(),
                    pm,
                    state,
                    thread: "A-7",
                }];
            }
            _ => return false,
        }
        true
    }
}

/// Composer keys after the input adapter decodes modifiers; Shift+Enter remains a text edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Escape closes the model list before closing Ask.
    Escape,
    /// Enter without Shift submits.
    Send,
    /// Ctrl/Command+Shift+M cycles configured models.
    CycleModel,
    /// Ctrl/Command+Shift+P toggles the app-owned target.
    ToggleTarget,
}

/// App-owned changes requested by a composer key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyOutcome {
    /// Effects to execute after the reducer.
    pub effects: Vec<Effect>,
    /// New target when the target shortcut was used.
    pub target: Option<AskTarget>,
    /// Clear the app's draft after a nonblank submission.
    pub clear_draft: bool,
}

/// Commands attached to content; the renderer need not infer behavior from button labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open Ask with optional replacement context.
    OpenAsk(Option<&'static str>),
    /// Toggle model list visibility.
    ToggleModelMenu,
    /// Select a configured profile.
    PickModel(&'static str),
    /// Remove bounded context.
    Unchip,
    /// Close Ask and clear its chip/list.
    CloseAsk,
    /// Open A-7 in Agents.
    OpenQuickThread,
    /// Open another assistant session.
    OpenThread(&'static str),
    /// Submit the app's draft to its selected target.
    Send,
    /// Submit a named session's own composer without affecting the quick-thread target.
    SendToThread(&'static str),
    /// Change the studio host state; Answering resumes streaming.
    SetMode(Mode),
    /// File pending messages without a model.
    FileNow,
    /// Withdraw pending messages.
    Withdraw,
    /// Enter or leave filed-field editing.
    Edit(Option<&'static str>),
    /// Set or close the share sheet step.
    Share(Option<ShareStep>),
    /// Set global-hotkey content.
    Hotkey(HotkeyState),
    /// Open the main window's quick thread.
    HotkeyOpen,
    /// Show an illustrative outcome.
    Toast(&'static str),
    /// Prompt for a merge destination.
    Merge(&'static str),
    /// Ask the app for fresh device confirmation.
    Confirm(Risk, Option<&'static str>),
    /// Open a linked record.
    Open(Page),
    /// Answer a write-permission prompt.
    Grant(Grant),
}

#[cfg(test)]
mod tests;
