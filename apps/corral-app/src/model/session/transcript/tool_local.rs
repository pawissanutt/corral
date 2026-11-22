//! Rows for the local tools, as `tool-lines-local.ts` words them: `diagnostics <path> · 2 errors
//! · 1 warning` (`✗` with errors, the first 3 serious items as rows), `definition` and
//! `references` with `→ path:line` or counts, `fetch`, `web read` and `search` with the site,
//! byte ranges or result counts, `browse` and `screenshot` with the page title or the PNG size,
//! `explore` and `oracle` with the prompt's first line and the headline of their findings
//! (`partial · stopped: time limit` when they stopped early), `tool search`, and an MCP server's
//! own tools under `server: title` with the result's line count.

use serde_json::Value;

use crate::model::hue::Hue;

use super::tool::{
    Call, CallState, Row, ToolState, View, count_of, head, list_of, plural, rows_of, str_of, text,
};

pub(super) fn present(call: &Call<'_>) -> Option<Row> {
    Some(match call.name {
        "lsp_diagnostics" => diagnostics(call),
        "lsp_definition" => locations(call, "definition"),
        "lsp_references" => locations(call, "references"),
        "web_fetch" => fetch(call),
        "web_read" => web_read(call),
        "web_search" => search(call),
        "browser_navigate" => browser(call, "browse"),
        "browser_screenshot" => browser(call, "screenshot"),
        "explore" => helper(call, "explore"),
        "oracle" => helper(call, "oracle"),
        "tool_search" => tool_search(call),
        name if name.starts_with("mcp_") => mcp(call),
        _ => return None,
    })
}

/// The JSON object the tool answered with, from its text.
fn json(call: &Call<'_>) -> Value {
    serde_json::from_str(call.text.trim()).unwrap_or(Value::Null)
}

/// The settled cases every local tool shares: a running or written call says nothing yet, a
/// failure its reason.
fn settled(call: &Call<'_>, row: Row) -> Result<Row, Row> {
    match call.state {
        CallState::Done => Ok(row),
        CallState::Failed => Err(row.failed_with(call.text)),
        _ => Err(row),
    }
}

/// `n lines` of a text result.
fn lines_words(call: &Call<'_>) -> String {
    plural(
        call.lines.unwrap_or(rows_of(call.text).len() as u64),
        "line",
        "lines",
    )
}

fn site(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    rest.trim_end_matches('/').to_owned()
}

fn diagnostics(call: &Call<'_>) -> Row {
    let row = Row::new(call, "diagnostics").target(
        call.arg("path")
            .map(|path| call.path(path))
            .unwrap_or_default(),
    );
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    let value = json(call);
    if !value.is_object() {
        let outcome = if rows_of(call.text).is_empty() {
            "no diagnostics"
        } else {
            "result in details"
        };
        row.expanded.extend(text("output", call.text));
        return row.ok(outcome);
    }
    let items = list_of(&value, "diagnostics");
    let total = count_of(&value, "total").unwrap_or(items.len() as u64);
    row.right = str_of(&value, "server").map(str::to_owned);
    if total == 0 {
        return row.ok(
            if value.get("complete").and_then(Value::as_bool) == Some(false) {
                "no diagnostics yet · server not settled"
            } else {
                "no diagnostics"
            },
        );
    }
    let severity = |item: &Value| str_of(item, "severity").unwrap_or("unknown").to_owned();
    let mut words = Vec::new();
    for (key, one, many) in [
        ("error", "error", "errors"),
        ("warning", "warning", "warnings"),
        ("information", "note", "notes"),
        ("hint", "hint", "hints"),
        ("unknown", "other", "others"),
    ] {
        let count = items.iter().filter(|item| severity(*item) == key).count() as u64;
        if count > 0 {
            words.push(plural(count, one, many));
        }
    }
    if total > items.len() as u64 {
        words.push(format!("{total} in all"));
    }
    let errors = items.iter().any(|item| severity(item) == "error");
    let warnings = items.iter().any(|item| severity(item) == "warning");
    let line = |item: &Value| {
        format!(
            "{}:{}  {}  {}",
            count_of(item, "line").map_or("?".to_owned(), |n| n.to_string()),
            count_of(item, "column").map_or("?".to_owned(), |n| n.to_string()),
            severity(item),
            head(str_of(item, "message").unwrap_or_default())
        )
    };
    row.body = items
        .iter()
        .filter(|item| matches!(severity(*item).as_str(), "error" | "warning"))
        .take(3)
        .map(line)
        .collect();
    row.expanded.extend(text(
        "diagnostics",
        &items.iter().map(line).collect::<Vec<_>>().join("\n"),
    ));
    let outcome = words.join(" · ");
    if errors {
        row.fail(outcome)
    } else {
        let mut row = row.ok(outcome);
        if warnings {
            row.hue = Hue::Held;
        }
        row
    }
}

fn locations(call: &Call<'_>, kind: &'static str) -> Row {
    let row = Row::new(call, kind).target(
        call.arg("path")
            .map(|path| call.path(path))
            .unwrap_or_default(),
    );
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    let value = json(call);
    let found = list_of(&value, "locations");
    let total = count_of(&value, "total").unwrap_or(found.len() as u64);
    let place = |item: &Value| {
        format!(
            "{}:{}",
            call.path(str_of(item, "path").unwrap_or("?")),
            count_of(item, "line").map_or("?".to_owned(), |n| n.to_string())
        )
    };
    let Some(first) = found.first() else {
        return row.ok(if kind == "definition" {
            "no definition found"
        } else {
            "no references"
        });
    };
    row.expanded.extend(text(
        "locations",
        &found
            .iter()
            .map(|item| {
                format!(
                    "{}  {}",
                    place(item),
                    str_of(item, "preview").unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    ));
    if kind == "definition" {
        row.detail = str_of(first, "preview").map(str::to_owned);
        return row.ok(if total == 1 {
            format!("→ {}", place(first))
        } else {
            format!("{total} locations · first {}", place(first))
        });
    }
    let mut files: Vec<&str> = found
        .iter()
        .filter_map(|item| str_of(item, "path"))
        .collect();
    files.sort_unstable();
    files.dedup();
    row.detail = Some(format!("first {}", place(first)));
    row.ok(format!(
        "{} in {}",
        plural(total, "reference", "references"),
        plural(files.len() as u64, "file", "files")
    ))
}

fn fetch(call: &Call<'_>) -> Row {
    let row = Row::new(call, "fetch").target(call.arg("url").map(site).unwrap_or_default());
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    let value = json(call);
    row.expanded.extend(text("output", call.text));
    if !value.is_object() {
        let outcome = lines_words(call);
        return row.ok(outcome);
    }
    let facts: Vec<String> = [
        count_of(&value, "status").map(|status| status.to_string()),
        str_of(&value, "contentType").map(str::to_owned),
        count_of(&value, "bytes").map(super::replay::size),
    ]
    .into_iter()
    .flatten()
    .collect();
    row.ok(if facts.is_empty() {
        "fetched".to_owned()
    } else {
        facts.join(" · ")
    })
}

fn web_read(call: &Call<'_>) -> Row {
    let target = call
        .arg("url")
        .map(site)
        .or_else(|| {
            call.arg("id")
                .map(|id| format!("snapshot {}", id.chars().take(8).collect::<String>()))
        })
        .unwrap_or_default();
    let row = Row::new(call, "web read").target(target);
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    let value = json(call);
    row.expanded.extend(text("output", call.text));
    let (Some(from), Some(length)) = (count_of(&value, "offset"), count_of(&value, "length"))
    else {
        let outcome = lines_words(call);
        return row.ok(outcome);
    };
    let total = count_of(&value, "total")
        .map(|total| format!(" of {total}"))
        .unwrap_or_default();
    let more = if value.get("more").and_then(Value::as_bool) == Some(true) {
        "more to read"
    } else {
        "end"
    };
    row.ok(format!("bytes {from}-{}{total} · {more}", from + length))
}

fn search(call: &Call<'_>) -> Row {
    let row = Row::new(call, "search").target(
        call.arg("query")
            .map(|query| format!("\"{query}\""))
            .unwrap_or_default(),
    );
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    let value = json(call);
    row.expanded.extend(text("output", call.text));
    if !value.is_object() {
        let answered = !call.text.trim().is_empty();
        row.detail = answered.then(|| head(call.text).to_owned());
        return row.ok(if answered { "answered" } else { "no answer" });
    }
    if value.get("configured").and_then(Value::as_bool) == Some(false) {
        let mut row = row.fail("search not configured");
        row.body = str_of(&value, "setup")
            .map(str::to_owned)
            .into_iter()
            .collect();
        return row;
    }
    let results = list_of(&value, "results");
    if results.is_empty() {
        return row.ok("no results");
    }
    row.detail = results
        .first()
        .and_then(|first| str_of(first, "title"))
        .map(str::to_owned);
    row.ok(plural(results.len() as u64, "result", "results"))
}

fn browser(call: &Call<'_>, verb: &'static str) -> Row {
    let row = Row::new(call, verb).target(call.arg("url").map(site).unwrap_or_default());
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    let value = call.result();
    if verb == "browse" {
        let title = str_of(&value, "title")
            .map(|title| format!("\"{title}\""))
            .unwrap_or_else(|| "loaded".to_owned());
        return match count_of(&value, "lines") {
            Some(lines) => row.ok(format!(
                "{title} · {} of text",
                plural(lines, "line", "lines")
            )),
            None => row.ok(title),
        };
    }
    let mut words = vec![count_of(&value, "bytes").map_or_else(
        || "captured".to_owned(),
        |bytes| format!("{} PNG", super::replay::size(bytes)),
    )];
    if let Some(saved) = str_of(&value, "path") {
        words.push(format!("saved {}", call.path(saved)));
    }
    row.expanded.extend(text("output", call.text));
    row.ok(words.join(" · "))
}

/// Why a helper stopped, in words.
fn stop_words(stop: &str) -> String {
    match stop {
        "completed" => "completed",
        "turn_limit" => "turn limit",
        "time_reserve" => "time limit",
        "timeout" => "timed out",
        "cancelled" => "cancelled",
        "provider_error" => "provider error",
        "empty_response" => "empty response",
        "output_limit" => "output limit",
        "uncertain" => "outcome uncertain",
        other => return other.replace('_', " "),
    }
    .to_owned()
}

fn helper(call: &Call<'_>, kind: &'static str) -> Row {
    let preset: Vec<&str> = [call.arg("profile"), call.arg("effort")]
        .into_iter()
        .flatten()
        .collect();
    let mut row =
        Row::new(call, kind).target(head(call.arg("prompt").unwrap_or_default()).to_owned());
    row.right = (!preset.is_empty()).then(|| preset.join(" · "));
    row.view = Some(call.view(View::Agents));
    let details = &call.details;
    match call.state {
        CallState::Writing | CallState::NotRun => return row,
        CallState::Running => {
            let progress = details.get("progress").unwrap_or(&Value::Null);
            let counts: Vec<String> = [
                count_of(progress, "turns").map(|turns| format!("turn {turns}")),
                count_of(progress, "tools").map(|tools| plural(tools, "tool", "tools")),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !counts.is_empty() {
                row.right = Some(counts.join(" · "));
            }
            let latest = head(call.text);
            if !latest.is_empty() {
                row.body = vec![latest.to_owned()];
            }
            return row;
        }
        CallState::Failed if str_of(details, "error").is_some() => {
            return row.failed_with(str_of(details, "error").unwrap_or(call.text));
        }
        CallState::Failed if !details.is_object() => return row.failed_with(call.text),
        CallState::Done | CallState::Failed => {}
    }
    let findings = str_of(details, "text").map_or_else(
        || match call.text.split_once("\n\n") {
            Some((_, rest)) => rest.to_owned(),
            None => call.text.to_owned(),
        },
        str::to_owned,
    );
    let report = details.get("report").unwrap_or(&Value::Null);
    let status =
        str_of(report, "status").unwrap_or(if str_of(details, "outcome") == Some("completed") {
            "completed"
        } else {
            "partial"
        });
    let stop = str_of(report, "stopReason")
        .or_else(|| str_of(details, "outcome"))
        .unwrap_or_default();
    let headline = head(&findings)
        .trim_start_matches('#')
        .trim()
        .replace("**", "")
        .replace("__", "")
        .replace('`', "");
    row.hidden = Some(plural(rows_of(&findings).len() as u64, "line", "lines"));
    row.expanded.extend(text("findings", &findings));
    match status {
        "failed" => row.fail(format!("failed · stopped: {}", stop_words(stop))),
        "completed" => row.ok(if headline.is_empty() {
            "completed".to_owned()
        } else {
            headline
        }),
        _ => {
            let mut row = row.state(
                ToolState::Unknown,
                format!("partial · stopped: {}", stop_words(stop)),
            );
            row.detail = (!headline.is_empty()).then_some(headline);
            row
        }
    }
}

fn tool_search(call: &Call<'_>) -> Row {
    let row = Row::new(call, "tool search").target(
        call.arg("query")
            .map(|query| format!("\"{query}\""))
            .unwrap_or_default(),
    );
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    let value = call.result();
    let tools: Vec<&str> = list_of(&value, "tools")
        .iter()
        .filter_map(|tool| str_of(tool, "name").or_else(|| tool.as_str()))
        .collect();
    row.expanded.extend(text("output", call.text));
    if !value.is_object() {
        let first = head(call.text);
        return row.ok(if first.is_empty() { "no result" } else { first }.to_owned());
    }
    if tools.is_empty() {
        return row.ok("no matching tools");
    }
    row.detail = Some(tools.join(", "));
    row.ok(plural(tools.len() as u64, "tool", "tools"))
}

fn mcp(call: &Call<'_>) -> Row {
    let value = call.result();
    let control = match call.name {
        "mcp_connect" => Some("MCP connect"),
        "mcp_resources" => Some("MCP resources"),
        "mcp_read_resource" => Some("MCP read"),
        "mcp_activate" => Some("MCP activate"),
        _ => None,
    };
    let Some(verb) = control else {
        // A server's own tool: `server: tool` with its first argument and the result's lines.
        let rest = call.name.trim_start_matches("mcp_");
        let verb = match rest.split_once('_') {
            Some((server, tool)) => format!("{server}: {}", tool.replace('_', " ")),
            None => rest.replace('_', " "),
        };
        let row = Row::new(call, verb).target(super::tool::target(&call.args));
        let mut row = match settled(call, row) {
            Ok(row) => row,
            Err(row) => return row,
        };
        row.expanded.extend(text("output", call.text));
        let outcome = if call.text.trim().is_empty() {
            "done".to_owned()
        } else {
            lines_words(call)
        };
        return row.ok(outcome);
    };
    let row = Row::new(call, verb).target(
        call.arg("server")
            .or_else(|| call.arg("uri"))
            .unwrap_or_default(),
    );
    let mut row = match settled(call, row) {
        Ok(row) => row,
        Err(row) => return row,
    };
    row.expanded.extend(text("output", call.text));
    match call.name {
        "mcp_activate" => {
            let count = list_of(&value, "activated").len() as u64;
            row.ok(format!("activated {}", plural(count, "tool", "tools")))
        }
        "mcp_connect" if value.get("configured").and_then(Value::as_bool) == Some(false) => {
            let mut row = row.ok("MCP not configured");
            row.hue = Hue::Held;
            row
        }
        "mcp_connect" => {
            let count =
                count_of(&value, "total").unwrap_or(list_of(&value, "servers").len() as u64);
            row.ok(plural(count, "server", "servers"))
        }
        "mcp_resources" => {
            let count = list_of(&value, "resources").len() as u64;
            row.ok(plural(count, "resource", "resources"))
        }
        _ => {
            let parts = list_of(&value, "contents").len() as u64;
            row.ok(if parts > 0 {
                plural(parts, "part", "parts")
            } else {
                "read".to_owned()
            })
        }
    }
}
