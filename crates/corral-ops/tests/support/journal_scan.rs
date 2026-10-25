use std::path::Path;

use super::journal_allowlist::JOURNAL_ALLOWLIST;

#[derive(Debug)]
struct Token {
    text: String,
    line: usize,
}

// A bounded lexical scanner rather than a regex over function text: comments and
// strings cannot create calls, and balanced call ranges identify callback events.
fn tokens(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut at = 0;
    let mut line = 1;
    while at < bytes.len() {
        let start = at;
        let start_line = line;
        if bytes[at].is_ascii_whitespace() {
            line += usize::from(bytes[at] == b'\n');
            at += 1;
            continue;
        }
        if bytes[at..].starts_with(b"//") {
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        if bytes[at..].starts_with(b"/*") {
            at += 2;
            let mut depth = 1;
            while at < bytes.len() && depth > 0 {
                if bytes[at..].starts_with(b"/*") {
                    depth += 1;
                    at += 2;
                } else if bytes[at..].starts_with(b"*/") {
                    depth -= 1;
                    at += 2;
                } else {
                    line += usize::from(bytes[at] == b'\n');
                    at += 1;
                }
            }
            continue;
        }
        let raw_quote = if bytes[at] == b'r' {
            let mut quote = at + 1;
            while quote < bytes.len() && bytes[quote] == b'#' {
                quote += 1;
            }
            (quote < bytes.len() && bytes[quote] == b'"').then_some(quote)
        } else {
            None
        };
        if let Some(quote) = raw_quote {
            let hashes = quote - at - 1;
            at = quote + 1;
            while at < bytes.len() {
                if bytes[at] == b'"'
                    && bytes.get(at + 1..at + 1 + hashes) == Some(&bytes[start + 1..quote])
                {
                    at += 1 + hashes;
                    break;
                }
                line += usize::from(bytes[at] == b'\n');
                at += 1;
            }
        } else if bytes[at] == b'"' {
            at += 1;
            while at < bytes.len() {
                match bytes[at] {
                    b'\\' => at = (at + 2).min(bytes.len()),
                    b'"' => {
                        at += 1;
                        break;
                    }
                    byte => {
                        line += usize::from(byte == b'\n');
                        at += 1;
                    }
                }
            }
        } else if bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
        } else if bytes[at] == b'\''
            && (bytes.get(at + 2) == Some(&b'\'')
                || (bytes.get(at + 1) == Some(&b'\\') && bytes.get(at + 3) == Some(&b'\'')))
        {
            at += if bytes[at + 1] == b'\\' { 4 } else { 3 };
        } else {
            // Advance by a character to keep non-ASCII prose/string boundaries valid.
            at += source[at..].chars().next().unwrap().len_utf8();
        }
        result.push(Token {
            text: source[start..at].into(),
            line: start_line,
        });
    }
    result
}

fn end(tokens: &[Token], open: usize) -> usize {
    let (left, right) = match tokens[open].text.as_str() {
        "(" => ("(", ")"),
        "{" => ("{", "}"),
        "[" => ("[", "]"),
        _ => panic!("not an opening delimiter"),
    };
    let mut depth = 0;
    for (at, token) in tokens.iter().enumerate().skip(open) {
        if token.text == left {
            depth += 1;
        } else if token.text == right {
            depth -= 1;
            if depth == 0 {
                return at;
            }
        }
    }
    panic!("unbalanced source at line {}", tokens[open].line);
}

fn mutation(name: &str) -> bool {
    matches!(
        name,
        "update"
            | "create"
            | "create_initialized"
            | "create_batch"
            | "publish_staged"
            | "try_update"
            | "dispatch_update"
            | "update_head"
            | "try_update_head"
            | "dispatch_update_head"
            | "save"
    )
}

fn buffered(name: &str) -> bool {
    mutation(name)
        || matches!(
            name,
            "submit" | "submit_head" | "submit_operating_action" | "submit_operating_action_head"
        )
}

pub fn scan(path: &str, source: &str) -> Vec<String> {
    let ts = tokens(source);
    let mut findings = Vec::new();
    let mut at = 0;
    while at < ts.len() {
        // A cfg(test) module is fixture code, even when embedded in a production file.
        if ts[at].text == "#" && ts.get(at + 1).is_some_and(|t| t.text == "[") {
            let attribute_end = end(&ts, at + 1);
            let attribute: String = ts[at + 2..attribute_end]
                .iter()
                .map(|t| t.text.as_str())
                .collect();
            if attribute == "cfg(test)" {
                at = attribute_end + 1;
                while at < ts.len() && ts[at].text != "{" && ts[at].text != ";" {
                    at += 1;
                }
                if ts.get(at).is_some_and(|t| t.text == "{") {
                    at = end(&ts, at) + 1;
                }
                continue;
            }
        }
        if ts[at].text != "fn" {
            at += 1;
            continue;
        }
        let function = &ts[at + 1].text;
        let mut body = at + 2;
        while body < ts.len() && ts[body].text != "{" && ts[body].text != ";" {
            body += 1;
        }
        if body == ts.len() || ts[body].text == ";" {
            at = body + 1;
            continue;
        }
        let body_end = end(&ts, body);
        let mut writes = Vec::new();
        let mut callbacks = Vec::new();
        for call in body + 1..body_end.saturating_sub(2) {
            if buffered(ts[call].text.as_str())
                && ts[call - 1].text == ":"
                && ts[call + 1].text == "("
            {
                callbacks.push((call + 1, end(&ts, call + 1)));
            }
            if ts[call].text != "." || ts[call + 2].text != "(" {
                continue;
            }
            let name = ts[call + 1].text.as_str();
            if buffered(name) {
                let close = end(&ts, call + 2);
                callbacks.push((call + 2, close));
                if mutation(name) && (name != "save" || ts[call - 1].text == "store") {
                    writes.push((call, close));
                }
            }
            if name != "log_event"
                || callbacks
                    .iter()
                    .any(|&(open, close)| open < call && call < close)
            {
                continue;
            }
            let Some(&(write, _)) = writes.iter().find(|&&(_, close)| close < call) else {
                continue;
            };
            let close = end(&ts, call + 2);
            let mut argument = call + 3;
            while argument < close && ts[argument].text != "," {
                if matches!(ts[argument].text.as_str(), "(" | "{" | "[") {
                    argument = end(&ts, argument);
                }
                argument += 1;
            }
            let first = argument + 1;
            argument = first;
            while argument < close && ts[argument].text != "," {
                if matches!(ts[argument].text.as_str(), "(" | "{" | "[") {
                    argument = end(&ts, argument);
                }
                argument += 1;
            }
            let mut events = ts[first..argument]
                .iter()
                .filter(|token| token.text.starts_with('"'))
                .map(|token| token.text.trim_matches('"').to_owned())
                .collect::<Vec<_>>();
            // Resolve a locally selected event kind, so adding a third branch does
            // not inherit an exception granted to the original two events.
            if events.is_empty()
                && argument == first + 1
                && let Some(binding) = (body + 1..call.saturating_sub(3)).rev().find(|&index| {
                    ts[index].text == "let"
                        && ts[index + 1].text == ts[first].text
                        && ts[index + 2].text == "="
                })
            {
                let mut until = binding + 3;
                while until < call && ts[until].text != ";" {
                    if matches!(ts[until].text.as_str(), "(" | "{" | "[") {
                        until = end(&ts, until);
                    }
                    until += 1;
                }
                events.extend(
                    ts[binding + 3..until]
                        .iter()
                        .filter(|token| token.text.starts_with('"'))
                        .map(|token| token.text.trim_matches('"').to_owned()),
                );
            }
            if events.is_empty() {
                let expression: String = ts[first..argument]
                    .iter()
                    .map(|token| token.text.as_str())
                    .collect();
                events.push(format!("<{expression}>"));
            }
            for event in events {
                let allowed =
                    JOURNAL_ALLOWLIST
                        .iter()
                        .any(|&(file, name, kind, effect, reason)| {
                            path == file
                                && function == name
                                && event == kind
                                && !reason.is_empty()
                                && ts[write..call]
                                    .windows(2)
                                    .any(|pair| pair[0].text == effect && pair[1].text == "(")
                        });
                if !allowed {
                    findings.push(format!("{path}:{} {function}: {event}", ts[call].line));
                }
            }
        }
        at = body_end + 1;
    }
    findings
}

pub fn scan_tree(root: &Path, directory: &Path, findings: &mut Vec<String>) {
    let mut entries = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if !matches!(
                path.file_name().unwrap().to_str().unwrap(),
                "test_support" | "tests"
            ) {
                scan_tree(root, &path, findings);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let name = path.file_name().unwrap().to_str().unwrap();
            if name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
            findings.extend(scan(relative, &std::fs::read_to_string(&path).unwrap()));
        }
    }
}
