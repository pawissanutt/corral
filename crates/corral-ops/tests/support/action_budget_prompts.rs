//! The emitted variant's prompts against an independent reference (plan T-253 section 4): each
//! emitted section is rendered from the full task's stored rows, found complete in every action
//! prompt the pass wrote, and removed; what remains equals the base output exactly.
use std::collections::BTreeMap;

use corral_core::Verdict;
use serde_json::json;

use super::action_budget::{cut_packets, normalize, packet_failures};
use super::action_budget_checks::sections;
use super::action_budget_fixture::{Bench, Kind};

const FINDINGS: &str = "## Previous plan findings\n\n```json\n";

/// The expected rendering of each emitted section the kind's prompts show, from the full task.
pub(super) fn references(bench: &Bench, kind: Kind) -> BTreeMap<String, String> {
    let task = bench.ctx.store.get(&bench.id).unwrap();
    let mut out = BTreeMap::new();
    for (section, _) in sections(kind) {
        match *section {
            "comments" => {
                out.insert(
                    "comments".to_owned(),
                    normalize(&crate::comments::owner_comments(&task)),
                );
            }
            "review_findings" => {
                let attempt = review_attempt(&task);
                let findings = crate::review_packet::findings_of(&task, &attempt).unwrap();
                out.insert(
                    "review_findings".to_owned(),
                    serde_json::to_string(&findings).unwrap(),
                );
            }
            "plan_findings" => {
                let findings: Vec<_> = task
                    .plans
                    .iter()
                    .flat_map(|plan| {
                        plan.reviews
                            .iter()
                            .filter(|r| matches!(r.verdict, Some(v) if v != Verdict::Approve))
                            .map(move |r| {
                                json!({"plan": plan.id, "attempt": r.id, "reviewer": r.reviewer,
                                    "verdict": r.verdict, "notes": r.notes})
                            })
                    })
                    .collect();
                let text = serde_json::to_string_pretty(&findings).unwrap();
                out.insert("plan_findings".to_owned(), normalize(&text));
            }
            _ => {}
        }
    }
    out
}

/// The review child the pass launched: the packet excludes its own attempt.
fn review_attempt(task: &corral_core::Task) -> String {
    task.current_run()
        .and_then(|run| run.review_runs.last())
        .map(|child| child.id.clone())
        .unwrap_or_default()
}

/// The packet's `previous_findings` array in `text`, as JSON, with its byte range.
fn packet_findings(text: &str) -> Option<(std::ops::Range<usize>, serde_json::Value)> {
    let key = "\"previous_findings\": ";
    let start = text.find(key)? + key.len();
    let mut depth = 0usize;
    for (i, c) in text[start..].char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + i + 1;
                    return Some((start..end, serde_json::from_str(&text[start..end]).ok()?));
                }
            }
            _ => {}
        }
    }
    None
}

/// A prompt the action itself wrote: the worker's registered copy, the planner's contract or a
/// plan review attempt's prompt.
fn action_prompt(header: &str) -> bool {
    header.contains("prompts/")
        || header.ends_with("/prompt.md:")
        || header.ends_with("-plan-review.md:")
        || header.ends_with("/review.md:")
}

/// `output` with every reference section verified and removed from each action prompt.
fn strip(
    kind: Kind,
    output: &str,
    refs: &BTreeMap<String, String>,
    name: &str,
    failures: &mut Vec<String>,
) -> String {
    let mut blocks: Vec<(String, String)> = vec![(String::new(), String::new())];
    for line in output.split_inclusive('\n') {
        if line.starts_with("prompt ") && line.trim_end().ends_with(':') {
            blocks.push((line.trim_end().to_owned(), String::new()));
        }
        blocks.last_mut().unwrap().1.push_str(line);
    }
    let review = matches!(kind, Kind::PlanReviewPane | Kind::PlanReviewPm);
    for (header, text) in blocks.iter_mut().filter(|(h, _)| action_prompt(h)) {
        if let Some(comments) = refs.get("comments") {
            if comments.is_empty() {
                if text.contains("\n## Owner comments\n") {
                    failures.push(format!(
                        "{name} {header}: owner comments without stored comments"
                    ));
                }
            } else if text.matches(comments.as_str()).count() == 1 {
                *text = text.replacen(comments.as_str(), "", 1);
            } else {
                failures.push(format!(
                    "{name} {header}: owner comments differ from the stored rows"
                ));
            }
        }
        if let Some(findings) = refs.get("review_findings") {
            match packet_findings(text) {
                Some((range, shown)) => {
                    let expected: serde_json::Value = serde_json::from_str(findings).unwrap();
                    let (shown, expected) = (
                        normalize(&shown.to_string()),
                        normalize(&super::action_budget::mask_clock_ids(&expected.to_string())),
                    );
                    if shown != expected {
                        failures.push(format!(
                            "{name} {header}: previous findings differ from the stored rows:\n{shown:.400}\nexpected:\n{expected:.400}"
                        ));
                    }
                    text.replace_range(range, "<findings>");
                }
                None if header.ends_with("/review.md:") => {
                    failures.push(format!("{name} {header}: no review packet findings"));
                }
                None => {}
            }
        }
        if let Some(findings) = refs.get("plan_findings") {
            match text.find(FINDINGS) {
                Some(start) => {
                    let from = start + FINDINGS.len();
                    let end = from + text[from..].find("\n```").unwrap_or(0);
                    if text[from..end] != *findings {
                        failures.push(format!(
                            "{name} {header}: plan findings differ from the stored rows:\n{}\nexpected:\n{findings}",
                            &text[from..end]
                        ));
                    }
                    text.replace_range(from..end, "<findings>");
                }
                None if review && header.ends_with("-plan-review.md:") => {
                    failures.push(format!("{name} {header}: no plan findings section"));
                }
                None => {}
            }
        }
    }
    blocks.into_iter().map(|(_, text)| text).collect()
}

/// The emitted output holds every emitted section complete in each action prompt, and nothing
/// else differs from the base output.
pub(super) fn emitted_output_failures(
    kind: Kind,
    grew: &[&str],
    (base, base_refs): (&str, &BTreeMap<String, String>),
    (emitted, emitted_refs): (&str, &BTreeMap<String, String>),
) -> Vec<String> {
    let mut failures = Vec::new();
    let (base, base_packets) = cut_packets(base);
    let (emitted, emitted_packets) = cut_packets(emitted);
    if sections(kind).iter().any(|(s, _)| *s == "resume") {
        failures.extend(packet_failures(&base_packets, &emitted_packets));
    }
    // A section's rendering changes exactly when its emitted reader reads added rows.
    for (section, text) in emitted_refs {
        let changed = base_refs.get(section) != Some(text);
        if changed != grew.contains(&section.as_str()) {
            failures.push(format!(
                "{section}: stored rendering changed {changed}, emitted reader rows grew {}",
                !changed
            ));
        }
    }
    let base = strip(kind, &base, base_refs, "base", &mut failures);
    let emitted = strip(kind, &emitted, emitted_refs, "emitted", &mut failures);
    if base != emitted {
        let (b, e) = base
            .lines()
            .zip(emitted.lines())
            .find(|(b, e)| b != e)
            .unwrap_or(("<end>", "<end>"));
        failures.push(format!(
            "emitted remainder differs:\n    base:    {b:.240}\n    emitted: {e:.240}"
        ));
    }
    failures
}
