//! Cards are read from canonical records, never accepted as assertions from host events.
use super::super::{actions, excerpt};
use crate::Ctx;
use crate::agent::ops::owner_prompt;
use anyhow::{Context, Result};
use corral_core::assistant::threads::Message;
use corral_protocol::assistant::{EntryCard, EntryKind, PromptChoice, ThreadEntry};
use corral_protocol::detail::question::{QuestionAnswer, QuestionOption};

pub(super) struct Snapshot {
    session: String,
    questions: Vec<corral_core::agent::AgentQuestion>,
    prompts: Vec<owner_prompt::Prompt>,
    now: jiff::Timestamp,
}

pub(super) fn load(ctx: &Ctx, session: &str) -> Result<Snapshot> {
    Ok(Snapshot {
        session: session.into(),
        questions: ctx.store.get_assistant_container(session)?.state.questions,
        prompts: owner_prompt::list(&ctx.store, session)?,
        now: ctx.store.now(),
    })
}

fn card(
    message: &Message,
    id: String,
    rev: u64,
    at: String,
    kind: EntryKind,
    text: Option<String>,
    card: EntryCard,
) -> ThreadEntry {
    ThreadEntry {
        id,
        rev,
        at,
        typed_at: None,
        kind,
        device: None,
        via: Some("assistant".into()),
        text,
        state: None,
        model: message.model.clone(),
        cost_usd: None,
        tools: Vec::new(),
        links: Vec::new(),
        attachments: Vec::new(),
        card: Some(card),
    }
}

fn idea_criteria(body: &str) -> Vec<String> {
    let mut criteria: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        if line == "## Owner words (verbatim)" {
            break;
        }
        if line == "## Acceptance criteria" {
            criteria.clear();
            in_section = true;
            continue;
        }
        if line.starts_with("# ") || line.starts_with("## ") {
            in_section = false;
        }
        if !in_section {
            continue;
        }
        if let Some(text) = line.strip_prefix("- ") {
            if criteria.len() == 32 {
                break;
            }
            criteria.push(excerpt(text, 1024));
        } else if let Some(last) = criteria.last_mut() {
            let remaining = 1024usize.saturating_sub(last.len());
            if remaining > 1 {
                last.push('\n');
                last.push_str(&line[..line.floor_char_boundary(remaining - 1)]);
            }
        }
    }
    for criterion in &mut criteria {
        criterion.truncate(criterion.trim_end().len());
    }
    criteria
}

pub(super) fn append(
    ctx: &Ctx,
    snapshot: &Snapshot,
    message: &Message,
    entries: &mut Vec<ThreadEntry>,
) -> Result<()> {
    for prompt in &snapshot.prompts {
        if Some(prompt.input.as_str()) != message.input.as_deref() {
            continue;
        }
        let expired = prompt.state == "open" && prompt.expires_at <= ctx.store.now();
        let settled = prompt.state != "open" || expired;
        let choices = if settled {
            Vec::new()
        } else if prompt.kind == "confirmation" {
            vec![PromptChoice::Confirm, PromptChoice::Decline]
        } else {
            vec![
                PromptChoice::Once,
                PromptChoice::Hour,
                PromptChoice::Session,
                PromptChoice::Deny,
            ]
        };
        let preview = prompt
            .command
            .as_deref()
            .or_else(|| prompt.descriptor["preview"].as_str())
            .unwrap_or_default();
        let status = if expired { "expired" } else { &prompt.state };
        let text = if prompt.answer_cmd.is_some() {
            format!(
                "{} · {} · {:?} by {} via app:{}",
                prompt.kind, status, prompt.answer_choice, message.name, prompt.device
            )
        } else {
            format!("{} · {}", prompt.kind, status)
        };
        let mut entry = card(
            message,
            prompt.key.clone(),
            prompt.revision as u64,
            prompt.created.to_string(),
            EntryKind::Prompt,
            Some(text),
            EntryCard::Prompt {
                key: prompt.key.clone(),
                rev: prompt.revision,
                tool: prompt.descriptor["tool"].as_str().map(|s| excerpt(s, 128)),
                path: prompt.descriptor["path"].as_str().map(|s| excerpt(s, 1024)),
                preview: excerpt(preview, 6 * 1024),
                choices,
                needs_presence: true,
            },
        );
        if settled {
            entry.state = Some(if expired {
                corral_protocol::assistant::MessageState::Withdrawn
            } else {
                corral_protocol::assistant::MessageState::Delivered
            });
            if prompt.answer_cmd.is_some() {
                entry.device = Some(prompt.device.clone());
                entry.via = Some(format!("app:{}", prompt.device));
            }
        }
        entries.push(entry);
    }
    for question in &snapshot.questions {
        if question.asked.turn != message.turn
            || message.turn.is_none()
            || Some(question.asked.generation) != message.generation
        {
            continue;
        }
        let answer = question.answer.as_ref().map(|a| QuestionAnswer {
            by: "owner".into(),
            via: a
                .attribution
                .as_ref()
                .map(|a| a.via.clone())
                .unwrap_or_else(|| "pane".into()),
            device: a.attribution.as_ref().and_then(|a| a.device.clone()),
            at: a.at.to_string(),
            text: excerpt(&a.text, 2 * 1024),
            option: a.option.as_ref().map(|s| excerpt(s, 128)),
        });
        let options = question
            .options
            .iter()
            .map(|label| QuestionOption {
                label: excerpt(label, 128),
                next: None,
                unblocks: Vec::new(),
                cost: None,
                recommended: question.recommendation.as_deref() == Some(label.as_str()),
                evidence: None,
            })
            .collect();
        entries.push(card(
            message,
            question.key.clone(),
            question.revision as u64,
            question.asked.at.to_string(),
            EntryKind::Question,
            Some(question.text.clone()),
            EntryCard::Question {
                key: question.key.clone(),
                rev: question.revision,
                options,
                answer,
            },
        ));
        if !question.recorded_pending()
            && question.answer.is_none()
            && let Some(entry) = entries.last_mut()
        {
            entry.state = Some(corral_protocol::assistant::MessageState::Withdrawn);
        }
        for (index, followup) in question.followups.iter().enumerate() {
            entries.push(ThreadEntry {
                id: format!("{}/followup/{index}", question.key),
                rev: question.revision as u64,
                at: followup.at.to_string(),
                typed_at: None,
                kind: EntryKind::Message,
                device: followup.attribution.device.clone(),
                via: Some(followup.attribution.via.clone()),
                text: Some(excerpt(&followup.text, 8 * 1024)),
                state: None,
                model: None,
                cost_usd: None,
                tools: Vec::new(),
                links: Vec::new(),
                attachments: Vec::new(),
                card: None,
            });
        }
    }
    if let Some(input) = &message.input
        && let Some(filing) = actions::filing(ctx, &message.session, input)?
    {
        let task = ctx.store.head(&filing.task)?;
        entries.push(card(
            message,
            format!("{}/idea", message.id),
            message.rev,
            message.at.clone(),
            EntryKind::Idea,
            None,
            EntryCard::Idea {
                task: filing.task,
                title: task.title.clone(),
                labels: task.labels.clone(),
                criteria: idea_criteria(&task.body),
                duplicates: Vec::new(),
                words: excerpt(&message.text, 4 * 1024),
            },
        ));
    }
    Ok(())
}

pub(super) fn revision(snapshot: &Snapshot, base: u64) -> Result<u64> {
    let digest = crate::agent::payload_sha256(
        &serde_json::json!({"base":base,"session":snapshot.session,
        "questions":snapshot.questions.iter().map(|q| (&q.key,q.revision)).collect::<Vec<_>>(),
        "prompts":snapshot.prompts.iter().map(|p| (&p.key,p.revision,&p.state,p.expires_at <= snapshot.now)).collect::<Vec<_>>() }),
    );
    u64::from_str_radix(&digest[..16], 16).context("encoding thread revision")
}

#[cfg(test)]
mod tests {
    #[test]
    fn acceptance_section_excludes_owner_words_and_keeps_continuations() {
        let body = "## Goal\nA real task.\n\n## Acceptance criteria\n\
            - first criterion\n  with a continuation\n- second criterion\n\n\
            ## Owner words (verbatim)\n## Acceptance criteria\n- not a filed criterion";
        assert_eq!(
            super::idea_criteria(body),
            ["first criterion\n  with a continuation", "second criterion"]
        );
    }
}
