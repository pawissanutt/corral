//! Path guard (plan T-253, review-scratch): production code never joins `state`, `prompts` or
//! `artifacts` onto a path, nor names `.corral/state`, now that `Store::state_dir()`,
//! `prompts_dir()` and `artifacts_dir()` are gone. Records, blobs and the scratch root replace
//! them. Test code is exempt: files under `tests/`, `*tests.rs` files, modules declared under
//! `#[cfg(test)]` and inline `#[cfg(test)] mod` blocks. A line that must keep such a path (the
//! migration fence, the owner's template overrides, the legacy snapshot mirror) carries
//! `path-guard: allow (<reason>)`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const PATTERNS: [&str; 4] = [
    "join(\"state\")",
    "join(\"prompts\")",
    "join(\"artifacts\")",
    ".corral/state",
];
const ALLOW: &str = "path-guard: allow (";

/// Violations in `text` as `<file>:<line>: <text>`, skipping comments, allowed lines and inline
/// `#[cfg(test)]` module blocks.
fn scan(file: &str, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut test_attr = false;
    let mut skip_depth: Option<i64> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(depth) = skip_depth.as_mut() {
            *depth += braces(line);
            if *depth <= 0 {
                skip_depth = None;
            }
            continue;
        }
        if is_test_cfg(trimmed) {
            test_attr = true;
            continue;
        }
        if test_attr && trimmed.starts_with("mod ") && trimmed.ends_with('{') {
            test_attr = false;
            skip_depth = Some(braces(line));
            continue;
        }
        if !trimmed.starts_with("#[") {
            test_attr = false;
        }
        if trimmed.starts_with("//") || line.contains(ALLOW) {
            continue;
        }
        if PATTERNS.iter().any(|pattern| line.contains(pattern)) {
            found.push(format!("{file}:{}: {trimmed}", index + 1));
        }
    }
    found
}

fn braces(line: &str) -> i64 {
    let open = line.matches('{').count() as i64;
    let close = line.matches('}').count() as i64;
    open - close
}

fn is_test_cfg(trimmed: &str) -> bool {
    trimmed.starts_with("#[cfg(") && trimmed.contains("test") && !trimmed.contains("not(test")
}

/// Files declared as test-only modules from `file`: `#[cfg(test)]` (or `all(test, ...)`)
/// followed by an optional `#[path = "..."]` and `mod name;`.
fn test_modules(file: &Path, text: &str) -> Vec<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new(""));
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let module_dir = if matches!(stem, "mod" | "lib" | "main") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    let mut out = Vec::new();
    let mut test_attr = false;
    let mut path_attr: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if is_test_cfg(line) {
            test_attr = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("#[path = \"") {
            path_attr = rest.split('"').next().map(str::to_owned);
            continue;
        }
        if test_attr
            && let Some(name) = line
                .strip_prefix("mod ")
                .or_else(|| line.strip_prefix("pub(crate) mod "))
                .and_then(|rest| rest.strip_suffix(';'))
        {
            match &path_attr {
                Some(path) => out.push(dir.join(path)),
                None => {
                    out.push(module_dir.join(format!("{name}.rs")));
                    out.push(module_dir.join(name).join("mod.rs"));
                }
            }
        }
        if !line.starts_with("#[") {
            test_attr = false;
            path_attr = None;
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "tests") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn violations(workspace: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for krate in std::fs::read_dir(workspace.join("crates")).unwrap() {
        let src = krate.unwrap().path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    let texts: Vec<(PathBuf, String)> = files
        .into_iter()
        .map(|file| {
            let text = std::fs::read_to_string(&file).unwrap();
            (file, text)
        })
        .collect();
    let exempt: BTreeSet<PathBuf> = texts
        .iter()
        .flat_map(|(file, text)| test_modules(file, text))
        .collect();
    texts
        .iter()
        .filter(|(file, _)| {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            !name.ends_with("tests.rs") && !exempt.contains(file)
        })
        .flat_map(|(file, text)| {
            let rel = file.strip_prefix(workspace).unwrap_or(file);
            scan(&rel.display().to_string(), text)
        })
        .collect()
}

#[test]
fn guard_names_file_and_line_of_each_fixture() {
    let fixtures = [
        (
            "a.rs",
            "fn f(s: &Store) {\n    let p = s.dir().join(\"state\");\n}\n",
            2,
        ),
        (
            "b.rs",
            "\n\nlet p = dir.join(\"prompts\").join(\"run.md\");\n",
            3,
        ),
        ("c.rs", "let p = root.join(\"artifacts\");\n", 1),
        (
            "d.rs",
            "fn f() {}\nconst P: &str = \".corral/state/x.json\";\n",
            2,
        ),
    ];
    for (file, text, line) in fixtures {
        let found = scan(file, text);
        assert_eq!(found.len(), 1, "{file}: {found:?}");
        assert!(
            found[0].starts_with(&format!("{file}:{line}: ")),
            "{file} reports file and line: {found:?}"
        );
    }
}

#[test]
fn guard_skips_comments_allowed_lines_and_inline_test_modules() {
    let text = "// dir.join(\"state\") in a comment\n\
                let p = dir.join(\"state\"); // path-guard: allow (migration fence)\n\
                #[cfg(test)]\n\
                mod tests {\n\
                    fn f() { let _ = dir.join(\"prompts\"); }\n\
                }\n\
                let q = dir.join(\"artifacts\");\n";
    assert_eq!(
        scan("e.rs", text),
        vec!["e.rs:7: let q = dir.join(\"artifacts\");".to_string()]
    );
}

#[test]
fn guard_exempts_files_declared_as_test_modules() {
    let text = "#[cfg(all(test, unix))]\nmod fixture;\n#[cfg(test)]\n#[path = \"x_tests.rs\"]\nmod tests;\nmod live;\n";
    let found = test_modules(Path::new("src/delivery.rs"), text);
    assert!(found.contains(&PathBuf::from("src/delivery/fixture.rs")));
    assert!(found.contains(&PathBuf::from("src/x_tests.rs")));
    assert!(!found.iter().any(|path| path.ends_with("live.rs")));
}

#[test]
fn production_code_joins_no_legacy_state_prompts_or_artifacts_path() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace = workspace.canonicalize().unwrap();
    let found = violations(&workspace);
    assert!(
        found.is_empty(),
        "production code still joins a legacy .corral path:\n{}",
        found.join("\n")
    );
}
