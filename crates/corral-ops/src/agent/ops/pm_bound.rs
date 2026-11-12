//! Native PM cards must fit the wire bound without changing their authority.
use anyhow::Result;
use serde_json::{Value, json};

use crate::pm_next::PmAction;

fn size(value: &Value) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

fn too_large() -> anyhow::Error {
    super::super::call::Rejection::new(
        super::super::call::RejectCode::BadRequest,
        "read exceeds 8 KiB; card identity cannot fit",
    )
    .into()
}

fn project(action: &PmAction, limit: usize, items: usize, shortlist: usize) -> Result<Value> {
    let mut value = serde_json::to_value(action)?;
    let mut omitted = 0;
    let evidence: Vec<_> = action
        .evidence
        .iter()
        .take(items)
        .map(|text| {
            let total = text.chars().count();
            if total <= limit {
                return text.clone();
            }
            omitted += total - limit;
            let preview: String = text.chars().take(limit).collect();
            format!("{preview}… [truncated: {limit} of {total} chars]")
        })
        .collect();
    omitted += action
        .evidence
        .iter()
        .skip(items)
        .map(|s| s.chars().count())
        .sum::<usize>();
    value["evidence"] = json!(evidence);
    if !action.shortlist.is_empty() {
        value["shortlist"] = serde_json::to_value(&action.shortlist[..shortlist])?;
    }
    let follow_up = if let Some(task) = &action.task {
        json!([
            {"op":"read_show","payload":{"task":task,"section":"overview"}},
            {"op":"read_show","payload":{"task":task,"section":"automation"}},
        ])
    } else {
        json!([{"op":"read_brief","payload":{}}])
    };
    value["bounded"] = json!({
        "evidence_chars_omitted": omitted,
        "evidence_items_dropped": action.evidence.len() - items,
        "shortlist_entries_dropped": action.shortlist.len() - shortlist,
        "follow_up": follow_up,
    });
    Ok(value)
}

pub(super) fn bounded_next(action: &PmAction, budget: usize) -> Result<Value> {
    let original = serde_json::to_value(action)?;
    if size(&original)? <= budget {
        return Ok(original);
    }
    let mut limit = 2048;
    loop {
        let value = project(action, limit, action.evidence.len(), action.shortlist.len())?;
        if size(&value)? <= budget {
            return Ok(value);
        }
        if limit == 32 {
            break;
        }
        limit /= 2;
    }
    for items in (0..action.evidence.len()).rev() {
        let value = project(action, limit, items, action.shortlist.len())?;
        if size(&value)? <= budget {
            return Ok(value);
        }
    }
    for shortlist in (0..action.shortlist.len()).rev() {
        let value = project(action, limit, 0, shortlist)?;
        if size(&value)? <= budget {
            return Ok(value);
        }
    }
    Err(too_large())
}

pub(super) fn bounded_brief(action: &PmAction, mut brief: Value, budget: usize) -> Result<Value> {
    let original = serde_json::to_value(action)?;
    brief["next"] = original.clone();
    if size(&brief)? <= budget {
        return Ok(brief);
    }
    // Reserve the smallest valid card before selecting a prefix of questions.
    let minimal = project(action, 32, 0, 0)?;
    let reserve = size(&minimal)?.min(size(&original)?);
    brief["next"] = Value::Null;
    let questions = brief["questions"]
        .as_array_mut()
        .map(std::mem::take)
        .unwrap_or_default();
    let total = questions.len();
    let mut low = 0;
    let mut high = total;
    while low < high {
        let keep = low + (high - low).div_ceil(2);
        set_questions(&mut brief, &questions, keep);
        if size(&brief)?.saturating_sub(4) + reserve <= budget {
            low = keep;
        } else {
            high = keep - 1;
        }
    }
    set_questions(&mut brief, &questions, low);
    let overhead = size(&brief)?.saturating_sub(4); // replace JSON null, not its key
    brief["next"] = bounded_next(action, budget.saturating_sub(overhead))?;
    if size(&brief)? > budget {
        return Err(too_large());
    }
    Ok(brief)
}

fn set_questions(brief: &mut Value, questions: &[Value], keep: usize) {
    brief["questions"] = json!(&questions[..keep]);
    if keep < questions.len() {
        brief["questions_total"] = json!(questions.len());
        brief["questions_omitted"] = json!(questions.len() - keep);
    } else if let Some(object) = brief.as_object_mut() {
        object.remove("questions_total");
        object.remove("questions_omitted");
    }
}
