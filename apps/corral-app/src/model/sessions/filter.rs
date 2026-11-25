use super::SessionRow;

#[derive(Debug, PartialEq, Eq)]
enum Term {
    Word(String),
    Mode(String),
    State(String),
    Project(String),
    Model(String),
    Task(String),
    Pinned,
}

pub(super) struct Query(Vec<Term>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Form, Id, agents::source_tests::agent, daemon::Feed, health::daemon_env, sessions::row,
    };
    use corral_types::{Role, SessionStatus};

    #[test]
    fn free_words_search_across_fields_and_qualifiers_narrow_exact_ids() {
        let id = Id::from("T-1_R1");
        let mut feed = Feed::default();
        let mut worker = agent("T-1", "R1", Role::Worker);
        worker.status = SessionStatus::Blocked {
            excerpt: "Overwrite?".into(),
        };
        feed.agents.insert(id.clone(), worker);
        let mut row = row(&feed, &id, daemon_env(&feed, Form::Desktop)).unwrap();
        row.title = "Ship docs".into();
        row.line.project = Some("site".into());
        row.model_id = Some("provider/Model-X".into());
        row.branch = Some("topic/help".into());
        row.doing = "Overwrite?".into();
        let query = Query::parse(
            "T-1_R1 SHIP site Model-X help Overwrite MODE:WORKER state:blocked project:SITE model:PROVIDER/ task:T-1 is:pinned",
        );
        assert!(query.without_mode(&row, || true));
        assert!(!query.without_mode(&row, || false));
        assert!(query.mode("worker"));
        assert!(!query.mode("reviewer"));
        for term in ["project:sit", "task:T-", "state:idle", "model:", "missing"] {
            assert!(!Query::parse(term).without_mode(&row, || true), "{term}");
        }
    }
}

impl Query {
    pub(super) fn parse(input: &str) -> Self {
        Self(
            input
                .split_whitespace()
                .map(|word| {
                    let word = word.to_lowercase();
                    match word.split_once(':') {
                        Some(("mode", value)) => Term::Mode(value.into()),
                        Some(("state", value)) => Term::State(value.into()),
                        Some(("project", value)) => Term::Project(value.into()),
                        Some(("model", value)) => Term::Model(value.into()),
                        Some(("task", value)) => Term::Task(value.into()),
                        Some(("is", "pinned")) => Term::Pinned,
                        _ => Term::Word(word),
                    }
                })
                .collect(),
        )
    }

    pub(super) fn without_mode(&self, row: &SessionRow, pinned: impl FnOnce() -> bool) -> bool {
        let contains =
            |text: &str, value: &str| !value.is_empty() && text.to_lowercase().contains(value);
        let pinned = self.0.contains(&Term::Pinned).then(pinned);
        self.0.iter().all(|term| match term {
            Term::Mode(_) => true,
            Term::State(value) => row.line.state.slug() == value,
            Term::Project(value) => row
                .line
                .project
                .as_ref()
                .is_some_and(|p| p.eq_ignore_ascii_case(value)),
            Term::Model(value) => row.model_id.as_ref().is_some_and(|m| contains(m, value)),
            Term::Task(value) => row
                .line
                .task
                .as_ref()
                .is_some_and(|t| t.as_str().eq_ignore_ascii_case(value)),
            Term::Pinned => pinned == Some(true),
            Term::Word(word) => [
                Some(row.line.id.as_str()),
                Some(row.shown_id.as_str()),
                Some(row.title.as_str()),
                row.line.project.as_deref(),
                row.model_id.as_deref(),
                row.branch.as_deref(),
                Some(row.doing.as_str()),
            ]
            .into_iter()
            .flatten()
            .any(|text| contains(text, word)),
        })
    }

    pub(super) fn mode(&self, role: &str) -> bool {
        self.0
            .iter()
            .all(|term| !matches!(term, Term::Mode(value) if value != role))
    }

    pub(super) fn without_mode_tokens(input: &str) -> String {
        input
            .split_whitespace()
            .filter(|word| {
                !word
                    .split_once(':')
                    .is_some_and(|(key, _)| key.eq_ignore_ascii_case("mode"))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(super) fn selected_mode(&self) -> Option<&str> {
        self.0.iter().find_map(|term| match term {
            Term::Mode(value) if self.mode(value) => Some(value.as_str()),
            _ => None,
        })
    }

    pub(super) fn has_mode(&self) -> bool {
        self.0.iter().any(|term| matches!(term, Term::Mode(_)))
    }
}
