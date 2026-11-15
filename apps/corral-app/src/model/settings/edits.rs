//! The daemon session's owner edits in Settings: the command identity behind each control (so
//! its truthful status shows beside it and is never re-minted after an uncertain result), the
//! inline fresh confirmation for device changes, the profile model being edited, the profile
//! being added, and the autopilot grant being written.

use std::collections::BTreeMap;

use corral_protocol::rows::SettingsSnapshot;

use super::State;
use super::autopilot::{Field, GrantForm, ModeVerb, Power};
use super::live::config_set;
use super::profiles::{self, ProfileField, ProfileForm};
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::{Act, ActStatus, Id, act};

/// A device change waiting for this device's fresh confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Revoke {
        device: String,
        name: String,
    },
    Instruct {
        device: String,
        name: String,
        on: bool,
    },
    Admin {
        device: String,
        name: String,
        on: bool,
    },
}

impl Pending {
    pub fn device(&self) -> &str {
        match self {
            Pending::Revoke { device, .. }
            | Pending::Instruct { device, .. }
            | Pending::Admin { device, .. } => device,
        }
    }

    /// The question the confirmation asks.
    pub fn question(&self) -> String {
        match self {
            Pending::Revoke { name, .. } => {
                format!("Revoke {name}? Its sessions end at once and it must be paired again.")
            }
            Pending::Instruct { name, on: true, .. } => {
                format!("Let what you type on {name} instruct assistants to act for you?")
            }
            Pending::Instruct {
                name, on: false, ..
            } => format!(
                "Stop what you type on {name} from instructing assistants? It can still ask and file."
            ),
            Pending::Admin { name, on: true, .. } => format!(
                "Make {name} an administrator? It will be able to change other devices' authority, including making more administrators, and to revoke devices."
            ),
            Pending::Admin {
                name, on: false, ..
            } => format!(
                "Remove {name}'s administrator authority? It can no longer change or revoke devices."
            ),
        }
    }

    pub fn confirm_label(&self) -> &'static str {
        match self {
            Pending::Revoke { .. } => "Revoke",
            Pending::Instruct { on: true, .. } => "Turn on",
            Pending::Instruct { on: false, .. } => "Turn off",
            Pending::Admin { on: true, .. } => "Make administrator",
            Pending::Admin { on: false, .. } => "Remove administrator",
        }
    }

    /// The device row's button that asks for this change; the two switches name what they
    /// switch so they are told apart when a row shows both.
    pub fn toggle_label(&self) -> &'static str {
        match self {
            Pending::Revoke { .. } => "Revoke",
            Pending::Instruct { on: true, .. } => "Allow instructing",
            Pending::Instruct { on: false, .. } => "Stop instructing",
            Pending::Admin { on: true, .. } => "Make admin",
            Pending::Admin { on: false, .. } => "Remove admin",
        }
    }
}

/// Owner edits in progress.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Edits {
    /// Command identity per control slug.
    pub acts: BTreeMap<String, Id>,
    pub confirming: Option<Pending>,
    /// `device.set_authority` correlation per device id.
    pub authority: BTreeMap<String, String>,
    /// The profile whose model is being edited, and the draft.
    pub model: Option<(String, String)>,
    /// The profile being added, while its form is open.
    pub adding: Option<ProfileForm>,
    pub grant: GrantForm,
    /// Whether the grant form is open.
    pub granting: bool,
}

/// The control slot of the profile add.
pub const ADD_PROFILE: &str = "profile-add";

/// The control slot of `profile`'s removal.
pub fn remove_slot(profile: &str) -> String {
    format!("remove-{profile}")
}

impl State {
    fn track(&mut self, slot: String, act: Act) -> Vec<Effect> {
        self.edits.acts.insert(slot, act.key.clone());
        vec![Effect::Act(act)]
    }

    /// The command `slot` last sent, while corrald has not said how it ended: asked about
    /// again under its own identity rather than sent anew.
    fn unsettled(&self, feed: &Feed, slot: &str) -> Option<Id> {
        let key = self.edits.acts.get(slot)?;
        act::status(feed, key)
            .is_some_and(|status| status.uncertain())
            .then(|| key.clone())
    }

    /// Open the add form copying the default source, or keep the form being written; after
    /// an add is done the next one starts afresh.
    pub fn open_add(&mut self, settings: &SettingsSnapshot, feed: &Feed) -> Vec<Effect> {
        let done = self
            .edits
            .acts
            .get(ADD_PROFILE)
            .and_then(|key| act::status(feed, key))
            == Some(ActStatus::Done);
        if done {
            self.edits.acts.remove(ADD_PROFILE);
        }
        if done || self.edits.adding.is_none() {
            self.edits.adding = Some(ProfileForm {
                from: profiles::default_source(settings),
                ..ProfileForm::default()
            });
        }
        Vec::new()
    }

    pub fn close_add(&mut self) -> Vec<Effect> {
        self.edits.adding = None;
        Vec::new()
    }

    /// Copy `from` instead.
    pub fn add_source(&mut self, from: String) -> Vec<Effect> {
        if let Some(form) = &mut self.edits.adding {
            form.from = from;
        }
        Vec::new()
    }

    pub fn add_field(&mut self, field: ProfileField, value: String) -> Vec<Effect> {
        if let Some(form) = &mut self.edits.adding {
            form.set(field, value);
        }
        Vec::new()
    }

    /// Preview and confirm the add, guarded by the configuration it was read from; the form
    /// stays as written until the add is done.
    pub fn submit_add(&mut self, settings: &SettingsSnapshot, feed: &Feed) -> Vec<Effect> {
        if let Some(key) = self.unsettled(feed, ADD_PROFILE) {
            return vec![Effect::Recover(key)];
        }
        match self
            .edits
            .adding
            .as_ref()
            .and_then(|form| form.act(settings))
        {
            Some(act) => self.track(ADD_PROFILE.into(), act),
            None => Vec::new(),
        }
    }

    /// Preview and confirm removing configured `profile` through corrald, guarded by the
    /// configuration it was read from.
    pub fn remove_configured_profile(
        &mut self,
        settings: &SettingsSnapshot,
        feed: &Feed,
        profile: &str,
    ) -> Vec<Effect> {
        let slot = remove_slot(profile);
        if let Some(key) = self.unsettled(feed, &slot) {
            return vec![Effect::Recover(key)];
        }
        self.track(slot, profiles::remove_act(settings, profile))
    }

    /// Ask this device to confirm a device change.
    pub fn confirm_device(&mut self, pending: Pending) -> Vec<Effect> {
        self.edits.confirming = Some(pending);
        Vec::new()
    }

    /// Drop the device change waiting for confirmation.
    pub fn cancel_device(&mut self) -> Vec<Effect> {
        self.edits.confirming = None;
        Vec::new()
    }

    /// The confirmed device change was sent; `operation` correlates an authority change.
    pub fn device_sent(&mut self, operation: Option<String>) -> Vec<Effect> {
        if let (Some(pending), Some(operation)) = (self.edits.confirming.take(), operation) {
            self.edits
                .authority
                .insert(pending.device().to_owned(), operation);
        }
        Vec::new()
    }

    /// Make `profile` the assistant profile, guarded by the configuration it was read from.
    pub fn use_profile(&mut self, settings: &SettingsSnapshot, profile: &str) -> Vec<Effect> {
        let act = config_set(
            settings,
            format!("Use {profile} for assistants"),
            "assistant.agent".into(),
            profile.to_owned(),
        );
        self.track(format!("use-{profile}"), act)
    }

    /// Start editing a profile's model.
    pub fn edit_model(&mut self, profile: String, model: String) -> Vec<Effect> {
        self.edits.model = Some((profile, model));
        Vec::new()
    }

    pub fn draft_model(&mut self, text: String) -> Vec<Effect> {
        if let Some((_, draft)) = &mut self.edits.model {
            *draft = text;
        }
        Vec::new()
    }

    pub fn cancel_model(&mut self) -> Vec<Effect> {
        self.edits.model = None;
        Vec::new()
    }

    /// Save the edited model through the hash-guarded configuration command.
    pub fn save_model(&mut self, settings: &SettingsSnapshot) -> Vec<Effect> {
        let Some((profile, draft)) = self.edits.model.take() else {
            return Vec::new();
        };
        let model = draft.trim().to_owned();
        if model.is_empty() {
            self.edits.model = Some((profile, draft));
            return Vec::new();
        }
        let act = config_set(
            settings,
            format!("Set {profile}'s model to {model}"),
            format!("profiles.{profile}.model"),
            model,
        );
        self.track(format!("model-{profile}"), act)
    }

    /// Change the operating mode (confirmed against corrald's preview).
    pub fn change_mode(&mut self, verb: ModeVerb) -> Vec<Effect> {
        self.track(format!("mode-{}", verb.slug()), verb.act())
    }

    pub fn open_grant(&mut self, open: bool) -> Vec<Effect> {
        self.edits.granting = open;
        Vec::new()
    }

    pub fn grant_field(&mut self, field: Field, value: String) -> Vec<Effect> {
        self.edits.grant.set(field, value);
        Vec::new()
    }

    pub fn grant_power(&mut self, power: Power) -> Vec<Effect> {
        let on = &mut self.edits.grant.powers[power as usize];
        *on = !*on;
        Vec::new()
    }

    pub fn grant_include_new(&mut self) -> Vec<Effect> {
        self.edits.grant.include_new = !self.edits.grant.include_new;
        Vec::new()
    }

    /// Preview and confirm the written grant; the form stays as written until it settles.
    pub fn submit_grant(&mut self) -> Vec<Effect> {
        match self.edits.grant.act() {
            Some(act) => self.track("grant".into(), act),
            None => Vec::new(),
        }
    }
}
