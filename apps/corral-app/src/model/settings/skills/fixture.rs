use super::*;
use crate::model::daemon::ClientError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fixture {
    switches: BTreeMap<String, bool>,
    files: BTreeMap<String, PiSkillDetail>,
    serial: u64,
}

impl Fixture {
    fn catalog(&self, project: Option<&str>) -> PiSkillsCatalog {
        use PiSkillSource::*;
        let mut rows = Vec::new();
        let mut add = |name: &str, source, root: &str| {
            let path = format!("{root}/{name}/SKILL.md");
            rows.push(PiSkillRow {
                id: path.clone(),
                name: name.into(),
                source,
                path,
                editable: matches!(source, User | Project),
                enabled: self.switches.get(name).copied().unwrap_or(true),
                required: source == Role,
                shadowed: false,
            });
        };
        for name in [
            "debugging",
            "programming",
            "git-master",
            "frontend",
            "visual-qa",
            "simplify-and-cleanup",
        ] {
            add(name, Builtin, "/preview/bundled/skills");
        }
        for name in ["corral-plan", "corral-diagrams", "corral-worker"] {
            add(name, Role, "/preview/bundled/roles");
        }
        add("notes", User, "/preview/configured/skills");
        if let Some(project) = project {
            add(
                "project-checks",
                Project,
                &format!("/preview/{project}/.agents/skills"),
            );
        }
        PiSkillsCatalog {
            skills: rows,
            revision: self.serial.to_string(),
        }
    }

    fn detail(&self, row: &PiSkillRow) -> PiSkillDetail {
        self.files.get(&row.id).cloned().unwrap_or_else(|| PiSkillDetail {
            id: row.id.clone(),
            text: format!("---\nname: {}\ndescription: Sample skill for preview.\n---\n\n# {}\n\nThis is a sample Pi skill. Its Markdown is not read from your device.\n", row.name, row.name),
            revision: "0".into(),
        })
    }

    pub fn reply(&mut self, request: &Request) -> Operation {
        let (method, _) = request.wire();
        let mut reply = Operation {
            method: method.into(),
            loading: false,
            value: None,
            error: None,
            refusal: None,
        };
        match self.answer(request) {
            Ok(value) => reply.value = Some(value),
            Err(message) => {
                reply.error = Some(ClientError {
                    code: "invalid_params".into(),
                    message: message.into(),
                })
            }
        }
        reply
    }

    fn answer(&mut self, request: &Request) -> Result<serde_json::Value, &'static str> {
        use serde_json::json;
        match request {
            Request::List(req) => Ok(json!(self.catalog(req.project.as_deref()))),
            Request::Get(req) => self
                .catalog(req.project.as_deref())
                .skills
                .iter()
                .find(|row| row.id == req.id)
                .map(|row| json!(self.detail(row)))
                .ok_or("Skill not found."),
            Request::Put(req) => {
                let catalog = self.catalog(req.project.as_deref());
                let row = catalog
                    .skills
                    .iter()
                    .find(|row| row.id == req.id && editable(row))
                    .ok_or("This skill is not editable.")?;
                let old = self.detail(row);
                if old.revision != req.revision {
                    return Err("This skill changed; reload before saving.");
                }
                self.serial += 1;
                let detail = PiSkillDetail {
                    id: req.id.clone(),
                    text: req.text.clone(),
                    revision: self.serial.to_string(),
                };
                self.files.insert(detail.id.clone(), detail.clone());
                Ok(json!(detail))
            }
            Request::Toggle(req) => {
                let catalog = self.catalog(req.project.as_deref());
                if catalog.revision != req.revision {
                    return Err("The skill catalog changed; reload before switching.");
                }
                if !catalog
                    .skills
                    .iter()
                    .any(|row| row.name == req.name && !row.required && !row.shadowed)
                {
                    return Err("This skill cannot be switched.");
                }
                self.serial += 1;
                self.switches.insert(req.name.clone(), req.enabled);
                Ok(json!(self.catalog(req.project.as_deref())))
            }
        }
    }
}
