//! Static allowlist of the emitted readers, the checkpoint payload readers and the own-record
//! scope (T-331 action-heads, plan T-253 section 4 "Emitted history" and "History-cost
//! classes"): production code in corral-core and corral-ops calls them only where allowed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const EMITTED: [&str; 3] = [
    "emitted_comments",
    "emitted_findings",
    "emitted_obligations",
];
const CHECKPOINT: [&str; 2] = ["checkpoint_payload", "checkpoint_metas_newest_first"];

/// The four prompt builders: Owner comments, the review packet's previous findings, Previous
/// plan findings and the resume packet.
const BUILDERS: [&str; 4] = [
    "corral-ops/src/comments.rs",
    "corral-ops/src/review_packet.rs",
    "corral-ops/src/review_contracts.rs",
    "corral-ops/src/resume.rs",
];

/// Where the readers are defined or forwarded, not called.
const DEFINITIONS: [&str; 5] = [
    "corral-core/src/store/head.rs",
    "corral-core/src/store/head_keyed.rs",
    "corral-core/src/store/task_ref_head.rs",
    "corral-core/src/store/task_ref_task.rs",
    "corral-core/src/task_ref.rs",
];

/// The own-record scope's four entries; `refresh_current` is `refresh_active_usage`'s per-task
/// step, checked below to have no other caller.
const OWN_RECORDS: [(&str, &str); 5] = [
    ("corral-ops/src/resume.rs", "resume_packet"),
    ("corral-ops/src/resume.rs", "require_checkpoint"),
    ("corral-core/src/progress.rs", "save_checkpoint"),
    ("corral-ops/src/usage.rs", "refresh_active_usage"),
    ("corral-ops/src/usage.rs", "refresh_current"),
];

struct Call {
    file: String,
    function: String,
    name: &'static str,
    line: usize,
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && !path.to_string_lossy().ends_with("tests.rs")
        {
            out.push(path);
        }
    }
}

/// Production lines of a file: everything before its first `#[cfg(test)]` module.
fn production(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let end = lines
        .windows(2)
        .position(|w| w[0].trim().starts_with("#[cfg(test)]") && w[1].contains("mod "))
        .unwrap_or(lines.len());
    lines[..end].to_vec()
}

fn function_name(line: &str) -> Option<String> {
    let rest = line.trim_start();
    let rest = rest
        .strip_prefix("pub(crate) ")
        .or_else(|| rest.strip_prefix("pub(super) "))
        .or_else(|| rest.strip_prefix("pub "))
        .unwrap_or(rest);
    let name = rest.strip_prefix("fn ")?;
    Some(
        name.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .next()?
            .to_owned(),
    )
}

fn calls() -> Vec<Call> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for krate in ["corral-core", "corral-ops"] {
        sources(&root.join(krate).join("src"), &mut files);
    }
    let names: Vec<&'static str> = EMITTED
        .iter()
        .chain(&CHECKPOINT)
        .copied()
        .chain(["own_records"])
        .collect();
    let mut out = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let file = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let mut function = String::new();
        for (i, line) in production(&text).into_iter().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if let Some(name) = function_name(code) {
                function = name;
            }
            for name in &names {
                let called = code.contains(&format!(".{name}("))
                    || code.contains(&format!("::{name}("))
                    || code.contains(&format!(" {name}("));
                if called && !code.contains(&format!("fn {name}")) {
                    out.push(Call {
                        file: file.clone(),
                        function: function.clone(),
                        name,
                        line: i + 1,
                    });
                }
            }
        }
    }
    out
}

#[test]
fn emitted_checkpoint_and_own_record_readers_are_called_only_where_allowed() {
    let mut seen = BTreeSet::new();
    for call in calls() {
        let at = format!("{}:{} in {}", call.file, call.line, call.function);
        if DEFINITIONS.contains(&call.file.as_str()) {
            continue;
        }
        seen.insert(call.name);
        if EMITTED.contains(&call.name) {
            assert!(
                BUILDERS.contains(&call.file.as_str()),
                "{} outside the four prompt builders: {at}",
                call.name
            );
        } else if CHECKPOINT.contains(&call.name) {
            let checkpoint_paths = [
                ("corral-ops/src/resume.rs", "require_checkpoint"),
                ("corral-core/src/progress.rs", "save_checkpoint"),
            ];
            assert!(
                BUILDERS.contains(&call.file.as_str())
                    || checkpoint_paths.contains(&(call.file.as_str(), call.function.as_str())),
                "{} outside the builders, require_checkpoint and save_checkpoint: {at}",
                call.name
            );
        } else if call.file != "corral-core/src/work.rs" {
            assert!(
                OWN_RECORDS.contains(&(call.file.as_str(), call.function.as_str())),
                "work::own_records entered outside its four call sites: {at}"
            );
        }
    }
    assert!(
        seen.contains("own_records"),
        "the scan found no own_records call"
    );
    let usage = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../corral-ops/src/usage.rs"),
    )
    .unwrap();
    let mut function = String::new();
    for line in production(&usage) {
        if let Some(name) = function_name(line) {
            function = name;
        }
        if line.contains("refresh_current(") && !line.contains("fn refresh_current") {
            assert_eq!(
                function, "refresh_active_usage",
                "refresh_current has another caller"
            );
        }
    }
}
