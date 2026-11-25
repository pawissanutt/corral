use nsh_core::prelude::*;

use super::AppState;
use crate::model::{Destination, Overlay, agents, commands::Action};

impl AppState {
    /// Executes a model palette action after closing the palette.
    pub fn run_command(&self, action: Action) {
        self.close(Overlay::Palette);
        match action {
            Action::Go(destination) => self.go(destination),
            Action::Settings(section) => self.go_settings(section),
            Action::OpenLive(live) => self.open_live(live),
            Action::Project(id) => self.open_project(id),
            Action::AllProjects => self.set_scope(crate::model::projects::Scope::All),
            Action::PreviewNoProjects => self.preview_no_projects(),
            Action::Ask(text) => match text {
                Some(text) => self.open_ask_with(&text),
                None => self.open_ask(),
            },
            Action::NewBuild => {
                self.go(Destination::Sessions);
                let filter = untracked(|| self.scope_filter());
                self.reduce(self.agents, |state, feed, _, _| {
                    state.act(
                        agents::Action::Start(agents::StartAction::Open),
                        feed,
                        &filter,
                    )
                });
            }
            Action::NewAssistant => self.new_assistant(),
            Action::Agents => self.open_agents(),
            Action::SidePane => self.toggle_pane(),
            Action::Theme(theme) => self.set_theme(theme),
            Action::Data(data) => self.set_data(data),
            Action::Hotkey => self.hotkey.set(true),
            Action::Share => {
                self.lock.set(false);
                self.close(Overlay::Ask);
                self.share.set(true);
            }
            Action::Lock => {
                self.share.set(false);
                self.lock.set(true);
            }
            Action::Work(press) => self.press_work(press),
            Action::Session(action) => {
                self.reduce(self.session, |state, feed, _, _| state.act(action, feed))
            }
        }
    }
}
