//! Conservative repository guidance defaults; explicit configuration always wins.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::{ReviewPolicy, Store, Task};

use crate::Ctx;

pub(crate) fn effective_size_limits(
    ctx: &Ctx,
    task: &Task,
    repo: &Path,
    policy: &ReviewPolicy,
) -> Result<BTreeMap<String, usize>> {
    let path = repo.join("AGENTS.md");
    let guidance = match std::fs::read_to_string(&path) {
        Ok(text) => from_guidance(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let store = Store::open(ctx.store.root())?;
    let global = store.config.review.size_limits.as_ref();
    let project = task
        .project
        .as_ref()
        .and_then(|project| store.config.projects.get(project))
        .and_then(|project| project.review.size_limits.as_ref());
    let mut limits = policy.size_limits.clone();
    limits.extend(guidance);
    for configured in [global, project].into_iter().flatten() {
        limits.extend(configured.clone());
    }
    Ok(limits)
}

fn from_guidance(text: &str) -> BTreeMap<String, usize> {
    let mut limits = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    let mut ceiling_section = false;
    let mut fenced = false;
    for line in text.lines() {
        let line = line.trim().to_ascii_lowercase();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let words: Vec<_> = line
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        let has_ceiling = words.iter().any(|word| {
            matches!(
                *word,
                "ceiling" | "ceilings" | "limit" | "limits" | "maximum"
            )
        });
        if line.starts_with('#') {
            ceiling_section = has_ceiling
                && words
                    .iter()
                    .any(|word| matches!(*word, "module" | "file" | "size" | "sloc"));
        }
        // Do not guess from examples, conditional rules, scoped paths or historical values.
        if (!has_ceiling && !ceiling_section)
            || line.split_whitespace().any(|token| {
                let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/');
                token.contains('/') && !matches!(token, "ts/tsx" | "ts/tsx/svelte" | "js/jsx")
            })
            || words.iter().any(|word| {
                matches!(
                    *word,
                    "example" | "examples" | "previous" | "formerly" | "unless" | "if" | "not"
                )
            })
            || !words.iter().any(|word| matches!(*word, "sloc" | "loc"))
        {
            continue;
        }
        let numbers: Vec<usize> = words.iter().filter_map(|word| word.parse().ok()).collect();
        let [limit] = numbers.as_slice() else {
            continue;
        };
        if *limit == 0 {
            continue;
        }
        for (language, extensions) in [
            ("rust", &["rs"][..]),
            ("rs", &["rs"][..]),
            ("typescript", &["ts", "tsx"][..]),
            ("ts", &["ts"][..]),
            ("tsx", &["tsx"][..]),
            ("javascript", &["js", "jsx"][..]),
            ("js", &["js"][..]),
            ("jsx", &["jsx"][..]),
            ("svelte", &["svelte"][..]),
            ("python", &["py"][..]),
            ("py", &["py"][..]),
            ("go", &["go"][..]),
        ] {
            if !words.contains(&language) {
                continue;
            }
            for extension in extensions {
                if limits
                    .insert((*extension).to_string(), *limit)
                    .is_some_and(|previous| previous != *limit)
                {
                    ambiguous.insert((*extension).to_string());
                }
            }
        }
    }
    limits.retain(|extension, _| !ambiguous.contains(extension));
    limits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_language_ceilings_are_extracted_but_ambiguous_guidance_is_ignored() {
        // Given clear repository ceilings and ambiguous, example and scoped alternatives.
        let text = "\
- Rust module ceiling: 1000 SLOC per file.
- TypeScript module ceiling: 400 SLOC.
- Python module ceiling: 600 SLOC.
- Python module ceiling: 700 SLOC.
- Example: Go module ceiling: 200 SLOC.
- JavaScript module ceiling: 300 SLOC in apps/web.
```
Rust module ceiling: 20 SLOC.
```
";
        // When guidance is interpreted without consulting any host files.
        let limits = from_guidance(text);

        // Then only unambiguous repository-wide language values become defaults.
        assert_eq!(
            limits,
            BTreeMap::from([("rs".into(), 1000), ("ts".into(), 400), ("tsx".into(), 400)])
        );
    }

    #[test]
    fn module_size_heading_supplies_context_for_language_rows() {
        // Given a dedicated size section followed by an unrelated section.
        let text = "## Module size limits\n- Rust: 1000 SLOC\n- TS/TSX/Svelte: 400 SLOC\n\
                    ## Examples\n- Python: 200 SLOC\n";

        // When clear language rows are extracted.
        let limits = from_guidance(text);

        // Then section context ends at the next heading.
        assert_eq!(
            limits,
            BTreeMap::from([
                ("rs".into(), 1000),
                ("ts".into(), 400),
                ("tsx".into(), 400),
                ("svelte".into(), 400),
            ])
        );
    }
}
