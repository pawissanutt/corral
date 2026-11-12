//! PM-only context notes, committed atomically with the native receipt.
use super::super::{
    admission::{self, Admission, Mode},
    call::{CallResponse, RejectCode, Rejection},
    envelope::Envelope,
    receipts::{self, Commit},
};
use crate::Ctx;
use anyhow::Result;
use corral_core::agent::{
    AgentRole,
    pm_notes::{self, NoteAuthor, NoteMutation, PmNotes},
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(flatten)]
    action: Action,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Action {
    Mutation(NoteMutation),
    List(List),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    op: ListVerb,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ListVerb {
    List,
}

fn view(notes: &PmNotes) -> Result<Value> {
    let mut result = serde_json::to_value(notes)?;
    result["omitted"] = json!(0);
    if serde_json::to_vec(&result)?.len() <= 7000 {
        return Ok(result);
    }
    result["entries"] = json!([]);
    for note in notes
        .entries
        .iter()
        .rev()
        .filter(|n| n.superseded_by.is_none())
    {
        let mut candidate = result.clone();
        if let Some(entries) = candidate["entries"].as_array_mut() {
            entries.push(serde_json::to_value(note)?);
        }
        candidate["omitted"] =
            json!(notes.entries.len() - candidate["entries"].as_array().map_or(0, Vec::len));
        if serde_json::to_vec(&candidate)?.len() > 7000 {
            break;
        }
        result = candidate;
    }
    result["omitted"] =
        json!(notes.entries.len() - result["entries"].as_array().map_or(0, Vec::len));
    Ok(result)
}
pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    if envelope.identity.role != AgentRole::Pm {
        return Err(Rejection::new(RejectCode::OpNotAllowed, "notes belong to the PM").into());
    }
    let request: Request = super::payload(envelope, "notes list/add/supersede/remove")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    receipts::commit(ctx, &admitted, |_, _| {
        let notes = match request.action {
            Action::List(list) => {
                let _ = list.op;
                pm_notes::load(&ctx.store, "workspace")?
            }
            Action::Mutation(mutation) => pm_notes::apply(
                &ctx.store,
                "workspace",
                request.expected_revision,
                NoteAuthor::Pm,
                mutation,
            )
            .map_err(|error| Rejection::new(RejectCode::BadRequest, error.to_string()))?,
        };
        Ok(Commit {
            result: view(&notes)?,
            effect: None,
            post_pending: Vec::new(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{self as ts, committed, envelope, hello, send};
    use crate::agent::{BindTarget, tool_request_id};

    #[test]
    fn pm_notes_request_commits_once_and_lists_entry() {
        let fx = ts::fx();
        let host = ts::bind(
            &fx,
            &BindTarget::Pm {
                project: "corral".into(),
            },
            "p2",
            "pm",
        );
        committed(&hello(&fx, &host));
        let request = tool_request_id(&host.identity.session_id, "notes-add");
        let input = envelope(
            &host,
            "notes",
            &request,
            json!({"op":"add","kind":"hypothesis","text":"context"}),
            false,
        );
        let first = committed(&send(&fx, &input, "notes"));
        assert_eq!(first.result["revision"], 1);
        assert_eq!(send(&fx, &input, "notes").receipt(), Some(&first));
        let list_id = tool_request_id(&host.identity.session_id, "notes-list");
        let listed = committed(&send(
            &fx,
            &envelope(&host, "notes", &list_id, json!({"op":"list"}), false),
            "notes",
        ));
        assert_eq!(listed.result["entries"].as_array().unwrap().len(), 1);
        assert_eq!(listed.result["entries"][0]["text"], "context");
    }

    #[test]
    fn bounded_view_omits_entries_instead_of_refusing() {
        let notes = PmNotes {
            entries: (0..64)
                .map(|id| pm_notes::Note {
                    id: format!("n-{id}"),
                    kind: pm_notes::NoteKind::Hypothesis,
                    text: "x".repeat(1024),
                    provenance: None,
                    supersedes: None,
                    reason: None,
                    resume_when: None,
                    author: NoteAuthor::Pm,
                    created_at: jiff::Timestamp::now(),
                    superseded_by: None,
                })
                .collect(),
            ..PmNotes::default()
        };
        let result = view(&notes).unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() <= 7000);
        assert!(result["omitted"].as_u64().unwrap() > 0);
        assert_eq!(result["entries"][0]["id"], "n-63");
    }
}
