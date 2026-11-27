//! Skill Markdown and workspace switches use independent revisions. Drafts belong to a
//! store and a project context; changing tabs or sources does not send or discard them.
use std::collections::BTreeMap;
mod fixture;
pub use fixture::Fixture;

use crate::model::{
    Id,
    daemon::{Feed, Operation},
};
use corral_protocol::config::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    List(PiSkillsRequest),
    Get(PiSkillGet),
    Put(PiSkillPut),
    Toggle(PiSkillToggle),
}

impl Request {
    pub fn too_large(&self) -> bool {
        let Self::Put(put) = self else { return false };
        // Account for JSON escaping and reserve space for the method and envelope.
        serde_json::to_vec(put).map_or(true, |json| {
            json.len().saturating_add(2 * 1024) > corral_protocol::app::APP_CONTROL_MAX
        })
    }

    pub fn wire(&self) -> (&'static str, serde_json::Value) {
        use serde_json::json;
        match self {
            Self::List(request) => (PI_SKILLS, json!(request)),
            Self::Get(request) => (PI_SKILL_GET, json!(request)),
            Self::Put(request) => (PI_SKILL_PUT, json!(request)),
            Self::Toggle(request) => (PI_SKILL_TOGGLE, json!(request)),
        }
    }
}

pub type Context = (String, Option<String>);
pub type Editors = BTreeMap<Context, Editor>;

pub fn unavailable(feed: &Feed) -> Option<&'static str> {
    if !feed.ready() {
        Some("Connect to the daemon, or open Preview to try Skills.")
    } else if !feed.capable(PI_SKILLS_V1) {
        Some("This daemon does not support Pi skill management.")
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Editor {
    pub catalog: Option<PiSkillsCatalog>,
    pub selected: Option<String>,
    pub saved: Option<PiSkillDetail>,
    pub draft: String,
    pub pending: Option<(Id, Request)>,
    pub problem: Option<String>,
    pub review: bool,
}

impl Editor {
    pub fn row(&self) -> Option<&PiSkillRow> {
        let id = self.selected.as_ref()?;
        self.catalog
            .as_ref()?
            .skills
            .iter()
            .find(|row| &row.id == id)
    }

    pub fn dirty(&self) -> bool {
        self.saved
            .as_ref()
            .is_some_and(|saved| saved.text != self.draft)
    }

    pub fn close(&mut self) {
        if !self.dirty() && self.pending.is_none() {
            self.selected = None;
            self.saved = None;
            self.draft.clear();
            self.problem = None;
            self.review = false;
        }
    }

    pub fn discard(&mut self) {
        if self.pending.is_none()
            && let Some(saved) = &self.saved
        {
            self.draft.clone_from(&saved.text);
        }
    }

    pub fn failed(&mut self, message: String) {
        self.problem = Some(message);
        self.pending = None;
    }

    pub fn settle(&mut self, reply: Operation) {
        let Some((_, request)) = self.pending.take() else {
            return;
        };
        let accepted = match request {
            Request::List(_) | Request::Toggle(_) => {
                reply.decode::<PiSkillsCatalog>().map(|catalog| {
                    self.catalog = Some(catalog);
                    if self.row().is_none() {
                        self.close();
                    }
                })
            }
            Request::Get(PiSkillGet { id, .. }) | Request::Put(PiSkillPut { id, .. }) => reply
                .decode::<PiSkillDetail>()
                .filter(|detail| detail.id == id)
                .map(|detail| {
                    self.draft.clone_from(&detail.text);
                    self.saved = Some(detail);
                }),
        };
        self.problem = if accepted.is_some() {
            None
        } else {
            Some(
                reply
                    .error
                    .map(|error| error.message)
                    .unwrap_or_else(|| "The daemon did not return a valid Skills reply.".into()),
            )
        };
    }
}

pub fn source(source: PiSkillSource) -> &'static str {
    match source {
        PiSkillSource::Builtin => "Built-in coding skill",
        PiSkillSource::Role => "Role procedure",
        PiSkillSource::User => "Configured user skill",
        PiSkillSource::Project => "Project skill",
    }
}

pub fn status(row: &PiSkillRow) -> &'static str {
    if row.required {
        "Required"
    } else if row.shadowed {
        "Shadowed"
    } else if row.enabled {
        "Enabled"
    } else {
        "Disabled"
    }
}

pub fn editable(row: &PiSkillRow) -> bool {
    row.editable && matches!(row.source, PiSkillSource::User | PiSkillSource::Project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_shared_file_write_leaves_the_draft_and_original_revision_intact() {
        let mut fixture = Fixture::default();
        let list = Request::List(PiSkillsRequest { project: None });
        let catalog = fixture.reply(&list).decode::<PiSkillsCatalog>().unwrap();
        let row = catalog
            .skills
            .iter()
            .find(|row| row.source == PiSkillSource::User)
            .unwrap();
        let id = row.id.clone();
        let get = Request::Get(PiSkillGet {
            project: None,
            id: id.clone(),
        });
        let saved = fixture.reply(&get).decode::<PiSkillDetail>().unwrap();
        let put = |project, text| {
            Request::Put(PiSkillPut {
                project,
                id: id.clone(),
                text,
                revision: saved.revision.clone(),
            })
        };
        let external = format!("{}\nChanged in another scope.", saved.text);
        assert!(
            fixture
                .reply(&put(Some("other".into()), external.clone()))
                .error
                .is_none()
        );

        let draft = format!("{}\nUnsaved.", saved.text);
        let request = put(None, draft.clone());
        let mut editor = Editor {
            catalog: Some(catalog),
            selected: Some(id.clone()),
            saved: Some(saved.clone()),
            draft: draft.clone(),
            pending: Some((Id::from("stale"), request.clone())),
            ..Default::default()
        };
        editor.settle(fixture.reply(&request));
        assert!(editor.pending.is_none());
        assert!(editor.problem.is_some());
        assert!(editor.dirty());
        assert_eq!(editor.draft, draft);
        assert_eq!(editor.saved.as_ref(), Some(&saved));
        assert_eq!(
            fixture.reply(&get).decode::<PiSkillDetail>().unwrap().text,
            external
        );
    }
}
