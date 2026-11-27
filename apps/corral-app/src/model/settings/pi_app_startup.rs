//! The app-managed startup defaults, one for a new Ask/Chat and one for a new Build, saved
//! independently of each other and of the corral.toml draft. Each side has its own draft and its
//! own save; a save sends only that side's change under the revision last read, so an unsaved
//! edit to the other side survives the reply.
use corral_protocol::{
    app::ASSISTANT_V1,
    config::{PI_APP_STARTUP_V1, PiAppStartupChange, PiAppStartupConfig, PiAppStartupSelection},
};

use crate::model::{
    Id,
    assistant::{self as live, ModelChoice},
    daemon::{Feed, Operation},
};

pub const ASK_NOTE: &str = "Used for a new app Ask or Chat only. A chat that already exists, or a profile picked for one chat, keeps its own model and effort. Clear it to fall back to the daemon's default Pi profile.";
pub const BUILD_NOTE: &str = "Used for new app-managed Build sessions only. Until saved here, Build inherits the terminal startup model. Clear the field and save to use the built-in default. A model chosen at launch still wins.";
pub const ASK_CLEAR: &str = "Default Pi profile (not set)";
pub const ASK_UNAVAILABLE: &str = "App Ask and Chat need a configured Pi harness or profile.";

pub fn unavailable(feed: &Feed) -> Option<&'static str> {
    if !feed.ready() {
        Some("Connect to a supported daemon to edit these settings.")
    } else if !feed.capable(PI_APP_STARTUP_V1) {
        Some("This daemon does not expose the app startup settings.")
    } else {
        None
    }
}

/// The two independent defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Ask,
    Build,
}

impl Slot {
    fn saved_problem(self, saved: &PiAppStartupConfig) -> Option<&str> {
        match self {
            Self::Ask => saved.ask_problem.as_deref(),
            Self::Build => saved.build_problem.as_deref(),
        }
    }
}

/// How a profile reads in the selector: its name, model and effort.
pub fn describe(choice: &ModelChoice) -> String {
    match &choice.effort {
        Some(effort) => format!("{} — {} · {effort} effort", choice.name, choice.model),
        None => format!("{} — {} · default effort", choice.name, choice.model),
    }
}

pub fn ask_available(feed: &Feed) -> bool {
    feed.capable(ASSISTANT_V1)
        && feed
            .settings
            .as_ref()
            .is_some_and(|settings| settings.ask_chat_profile.is_some())
}

/// The selectable options use the same Pi profiles as the per-chat picker.
pub fn ask_options(feed: &Feed) -> Vec<(Option<String>, String)> {
    let mut options = vec![(None, ASK_CLEAR.to_owned())];
    if ask_available(feed) {
        options.extend(live::models(feed).into_iter().map(|choice| {
            let label = describe(&choice);
            (Some(choice.name), label)
        }));
    }
    options
}

/// Keep a deleted or repurposed profile visible as the current selection, not as an option.
pub fn ask_label(options: &[(Option<String>, String)], draft: Option<&str>) -> String {
    options
        .iter()
        .find(|(name, _)| name.as_deref() == draft)
        .map_or_else(
            || format!("{} — not an eligible Pi profile", draft.unwrap_or_default()),
            |(_, label)| label.clone(),
        )
}

/// What a new Ask/Chat is answering with now, as the daemon resolves it.
pub fn effective_ask(feed: &Feed) -> Option<String> {
    let name = feed.settings.as_ref()?.ask_chat_profile.clone()?;
    Some(
        live::models(feed)
            .iter()
            .find(|choice| choice.name == name)
            .map_or(name, describe),
    )
}

#[derive(Clone, PartialEq)]
struct Waiting {
    key: Id,
    /// None while reading.
    save: Option<Slot>,
    ask_at_send: Option<String>,
    build_at_send: String,
}

#[derive(Clone, Default, PartialEq)]
pub struct Editor {
    pub saved: Option<PiAppStartupConfig>,
    /// The selected profile, None for cleared.
    pub ask: Option<String>,
    pub build: String,
    waiting: Option<Waiting>,
    /// The slot a failed save belongs to, None for a failed read.
    problem: Option<(Option<Slot>, String)>,
}

impl Editor {
    pub fn pending(&self) -> Option<&Id> {
        self.waiting.as_ref().map(|waiting| &waiting.key)
    }

    fn saved_ask(&self) -> Option<&str> {
        self.saved.as_ref()?.ask_profile.as_deref()
    }

    fn saved_build(&self) -> &str {
        self.saved
            .as_ref()
            .and_then(|saved| saved.build_model.as_deref())
            .unwrap_or_default()
    }

    /// Whether this side's draft differs from what was read.
    pub fn dirty(&self, slot: Slot) -> bool {
        self.saved.is_some()
            && match slot {
                Slot::Ask => self.ask.as_deref() != self.saved_ask(),
                Slot::Build => self.build.trim() != self.saved_build(),
            }
    }

    /// The request that saves one side under the revision last read.
    pub fn change(&self, slot: Slot) -> Option<PiAppStartupChange> {
        let saved = self.saved.as_ref()?;
        let pin_inherited_build = slot == Slot::Build && saved.build_inherited;
        let repair = slot.saved_problem(saved).is_some();
        if self.waiting.is_some() || !(self.dirty(slot) || pin_inherited_build || repair) {
            return None;
        }
        let selection = match slot {
            Slot::Ask => PiAppStartupSelection::AskProfile {
                profile: self.ask.clone(),
            },
            Slot::Build => {
                let model = self.build.trim();
                PiAppStartupSelection::BuildModel {
                    model: (!model.is_empty()).then(|| model.to_owned()),
                }
            }
        };
        Some(PiAppStartupChange {
            revision: saved.revision.clone(),
            selection,
        })
    }

    pub fn sent(&mut self, key: Id, save: Option<Slot>, sent: bool) {
        self.waiting = sent.then(|| Waiting {
            key,
            save,
            ask_at_send: self.ask.clone(),
            build_at_send: self.build.clone(),
        });
        self.problem = (!sent).then(|| (save, "Could not send the request to the daemon.".into()));
    }

    /// Takes the daemon's reply. A side follows the daemon when it holds no unsaved edit (or
    /// is the side just saved and not edited since); an unsaved edit to the other side stays.
    pub fn settle(&mut self, reply: Operation) {
        let Some(waiting) = self.waiting.take() else {
            return;
        };
        let Some(value) = reply.decode::<PiAppStartupConfig>() else {
            self.problem = Some((
                waiting.save,
                reply.error.map_or_else(
                    || "The daemon returned no setting.".into(),
                    |error| error.message,
                ),
            ));
            return;
        };
        self.problem = None;
        let ask_follows = if waiting.save == Some(Slot::Ask) {
            self.ask == waiting.ask_at_send
        } else {
            !self.dirty(Slot::Ask)
        };
        let build_follows = if waiting.save == Some(Slot::Build) {
            self.build == waiting.build_at_send
        } else {
            !self.dirty(Slot::Build)
        };
        if ask_follows {
            self.ask.clone_from(&value.ask_profile);
        }
        if build_follows {
            self.build = value.build_model.clone().unwrap_or_default();
        }
        self.saved = Some(value);
    }

    /// The line under one side: what is in flight, what failed, or what is saved.
    pub fn status(&self, slot: Slot) -> (String, bool) {
        if let Some(waiting) = &self.waiting {
            match waiting.save {
                Some(saving) if saving == slot => return ("Saving…".into(), false),
                None => return ("Reading…".into(), false),
                Some(_) => {}
            }
        }
        if let Some((owner, problem)) = &self.problem
            && owner.is_none_or(|owner| owner == slot)
        {
            return (problem.clone(), true);
        }
        let Some(saved) = &self.saved else {
            return (String::new(), false);
        };
        if let Some(problem) = slot.saved_problem(saved) {
            return (problem.to_owned(), true);
        }
        let line = match slot {
            Slot::Ask => format!(
                "Saved: {}",
                saved.ask_profile.as_deref().unwrap_or("not set")
            ),
            Slot::Build => format!(
                "{}: {}",
                if saved.build_inherited {
                    "Inherited"
                } else {
                    "Saved"
                },
                saved.build_model.as_deref().unwrap_or("built-in default")
            ),
        };
        let line = if self.dirty(slot) {
            format!("{line} (unsaved edit)")
        } else {
            line
        };
        (line, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::ClientError;
    use serde_json::json;

    fn reply(value: Option<serde_json::Value>, error: Option<ClientError>) -> Operation {
        Operation {
            method: "config.pi_app_startup".into(),
            loading: false,
            value,
            error,
            refusal: None,
        }
    }

    fn read(editor: &mut Editor, revision: &str, ask: Option<&str>, build: Option<&str>) {
        editor.sent(Id::from("read".to_owned()), None, true);
        editor.settle(reply(
            Some(json!({"revision": revision, "ask_profile": ask, "build_model": build, "build_inherited": false})),
            None,
        ));
    }

    fn save(editor: &mut Editor, slot: Slot) -> PiAppStartupChange {
        let change = editor.change(slot).expect("a change");
        editor.sent(Id::from("save".to_owned()), Some(slot), true);
        change
    }

    #[test]
    fn each_side_saves_only_itself_and_keeps_the_other_drafts_edit() {
        let mut editor = Editor::default();
        editor.sent(Id::from("read".to_owned()), None, true);
        editor.settle(reply(
            Some(json!({"revision": "r1", "ask_profile": null, "build_model": "openai/legacy", "build_inherited": true})),
            None,
        ));
        assert_eq!(editor.build, "openai/legacy");
        assert!(editor.change(Slot::Ask).is_none());
        assert_eq!(
            editor.change(Slot::Build).unwrap().selection,
            PiAppStartupSelection::BuildModel {
                model: Some("openai/legacy".into())
            }
        );

        editor.ask = Some("pi-sol-high".into());
        editor.build = "openai/other".into();
        let change = save(&mut editor, Slot::Ask);
        assert_eq!(change.revision, "r1");
        assert_eq!(
            change.selection,
            PiAppStartupSelection::AskProfile {
                profile: Some("pi-sol-high".into())
            }
        );
        // One request at a time: the other side cannot save while this one is in flight.
        assert!(editor.change(Slot::Build).is_none());
        editor.settle(reply(
            Some(json!({"revision": "r2", "ask_profile": "pi-sol-high", "build_model": "openai/legacy", "build_inherited": true})),
            None,
        ));
        assert_eq!(editor.ask.as_deref(), Some("pi-sol-high"));
        assert_eq!(editor.build, "openai/other", "unsaved Build edit survives");
        assert!(!editor.dirty(Slot::Ask) && editor.dirty(Slot::Build));
        let change = editor.change(Slot::Build).unwrap();
        assert_eq!(change.revision, "r2");
        assert_eq!(
            change.selection,
            PiAppStartupSelection::BuildModel {
                model: Some("openai/other".into())
            }
        );
    }

    #[test]
    fn saving_build_keeps_an_unsaved_ask_edit_and_clear_is_explicit() {
        let mut editor = Editor::default();
        read(&mut editor, "r1", Some("pi-sol"), Some("openai/m"));
        editor.ask = Some("pi-sol-high".into());
        editor.build = "  ".into();
        let change = save(&mut editor, Slot::Build);
        assert_eq!(
            change.selection,
            PiAppStartupSelection::BuildModel { model: None }
        );
        editor.settle(reply(
            Some(json!({"revision": "r2", "ask_profile": "pi-sol", "build_model": null, "build_inherited": false})),
            None,
        ));
        assert_eq!(editor.build, "");
        assert_eq!(editor.ask.as_deref(), Some("pi-sol-high"));
        assert!(!editor.dirty(Slot::Build) && editor.change(Slot::Build).is_none());
        assert!(editor.status(Slot::Ask).0.ends_with("(unsaved edit)"));
        // Clearing Ask is a change of its own.
        editor.ask = None;
        assert_eq!(
            editor.change(Slot::Ask).unwrap().selection,
            PiAppStartupSelection::AskProfile { profile: None }
        );
    }

    #[test]
    fn a_later_edit_during_a_save_is_kept_and_a_stale_save_keeps_both_drafts() {
        let mut editor = Editor::default();
        read(&mut editor, "r1", None, None);
        editor.ask = Some("pi-sol-high".into());
        save(&mut editor, Slot::Ask);
        editor.ask = None;
        editor.settle(reply(
            Some(json!({"revision": "r2", "ask_profile": "pi-sol-high", "build_model": null, "build_inherited": false})),
            None,
        ));
        assert_eq!(
            editor.ask, None,
            "reverting during save keeps the new draft"
        );

        editor.ask = Some("pi-sol".into());
        editor.build = "openai/b".into();
        let change = save(&mut editor, Slot::Ask);
        editor.settle(reply(
            None,
            Some(ClientError {
                code: "invalid_params".into(),
                message: "pi.toml changed; reload before saving".into(),
            }),
        ));
        assert_eq!(
            editor.status(Slot::Ask),
            ("pi.toml changed; reload before saving".into(), true)
        );
        assert!(!editor.status(Slot::Build).1, "the error belongs to Ask");
        assert_eq!(editor.build, "openai/b");
        assert_eq!(editor.change(Slot::Ask), Some(change));
        // Reload refreshes the revision and keeps both drafts.
        editor.sent(Id::from("reload".to_owned()), None, true);
        editor.settle(reply(
            Some(json!({"revision": "r3", "ask_profile": "pi-sol-high", "build_model": null, "build_inherited": false})),
            None,
        ));
        assert_eq!(editor.ask.as_deref(), Some("pi-sol"));
        assert_eq!(editor.build, "openai/b");
        assert_eq!(editor.change(Slot::Ask).unwrap().revision, "r3");
    }

    #[test]
    fn a_bad_saved_value_can_be_cleared_without_disabling_the_other_setting() {
        let mut editor = Editor::default();
        editor.sent(Id::from("read".to_owned()), None, true);
        editor.settle(reply(
            Some(json!({"revision": "r1", "ask_profile": null, "build_model": "openai/build", "build_inherited": false, "ask_problem": "Invalid profile"})),
            None,
        ));
        assert!(editor.status(Slot::Ask).1 && !editor.status(Slot::Build).1);
        assert_eq!(
            editor.change(Slot::Ask).unwrap().selection,
            PiAppStartupSelection::AskProfile { profile: None }
        );
        editor.build.clear();
        assert_eq!(
            editor.change(Slot::Build).unwrap().selection,
            PiAppStartupSelection::BuildModel { model: None }
        );
    }

    #[test]
    fn options_list_eligible_pi_profiles_with_effort_and_keep_a_saved_one_visible() {
        let mut feed = Feed::default();
        feed.settings = Some(corral_protocol::rows::SettingsSnapshot {
            config: json!({"profiles": {
                "pi-sol": {"harness": "pi", "model": "openai/gpt-6.2-sol"},
                "pi-sol-high": {"harness": "pi", "model": "openai/gpt-6.2-sol", "effort": "high"},
                "claude": {"harness": "claude", "model": "opus"}
            }})
            .as_object()
            .unwrap()
            .clone(),
            ask_chat_profile: Some("pi-sol-high".into()),
            ..Default::default()
        });
        assert_eq!(
            ask_options(&feed).len(),
            1,
            "a disabled chat has only the clear choice"
        );
        feed.capabilities.insert(ASSISTANT_V1.into());
        let options = ask_options(&feed);
        let names: Vec<_> = options.iter().map(|(name, _)| name.as_deref()).collect();
        assert_eq!(names, [None, Some("pi-sol"), Some("pi-sol-high")]);
        assert!(options[2].1.contains("openai/gpt-6.2-sol"));
        assert!(options[2].1.contains("high"));
        assert_eq!(effective_ask(&feed).as_ref(), Some(&options[2].1));
        assert!(ask_label(&options, Some("gone")).contains("gone"));
    }
}
