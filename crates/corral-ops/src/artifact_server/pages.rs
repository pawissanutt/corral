//! What the artifact server shows: the URL layout, request paths resolved against recorded snapshots (never the
//! file system outside `.corral/artifacts`), the live header injected into documents, index pages and Markdown
//! rendered to HTML.
//!
//! An artifact is served at `a/<task>/<run>/<source layout>`: its original absolute source path without the
//! root, so a report's relative links (`img/a.png`, `../style.css`) land on the URLs of the sibling snapshots
//! recorded from those source files. Artifacts whose source cannot be mirrored use their snapshot name.

use std::path::Path;

use corral_core::{Artifact, Task};
use sha2::{Digest, Sha256};

use crate::Ctx;

pub(super) fn ext(a: &Artifact) -> String {
    crate::viewers::extension(Path::new(&a.path))
}

/// HTML and Markdown: what the index lists and what gets the live header.
pub(super) fn is_document(a: &Artifact) -> bool {
    matches!(ext(a).as_str(), "html" | "htm" | "md" | "markdown")
}

/// Task id from "artifacts/<task>/<run>/<file>".
pub(super) fn task_of(a: &Artifact) -> Option<&str> {
    a.path.split('/').nth(1)
}

/// The path below `a/<task>/<run>/` an artifact is served at (decoded segments joined with '/').
pub(super) fn rel_path(a: &Artifact) -> String {
    match crate::artifact_storage::source_layout(&a.source) {
        Some(p) => p
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        None => corral_core::snapshot_name(a).to_string(),
    }
}

/// URL path (after `/<token>/`) of an artifact, percent-encoded.
pub fn artifact_url_path(a: &Artifact) -> Option<String> {
    let task = task_of(a)?;
    let rel: Vec<String> = rel_path(a).split('/').map(encode).collect();
    Some(format!(
        "a/{}/{}/{}",
        encode(task),
        encode(&a.run),
        rel.join("/")
    ))
}

fn source_name(a: &Artifact) -> Option<&str> {
    Path::new(&a.source).file_name().and_then(|n| n.to_str())
}

/// The snapshot a request for `rel` under `run` names: its snapshot name in that run, then the newest
/// snapshot of exactly that source path in the run, then in any run of the task (assets recorded earlier).
/// Nothing else matches: a missing asset is a 404, never a same-named file from another directory.
pub(super) fn resolve<'a>(task: &'a Task, run: &str, rel: &str) -> Option<&'a Artifact> {
    let arts = &task.artifacts;
    arts.iter()
        .rev()
        .find(|a| a.run == run && corral_core::snapshot_name(a) == rel)
        .or_else(|| {
            arts.iter()
                .rev()
                .find(|a| a.run == run && rel_path(a) == rel)
        })
        .or_else(|| arts.iter().rev().find(|a| rel_path(a) == rel))
}

/// The last recorded live snapshot of the same document as `served`: same source path, or same title and file name
/// (a later run's worktree may live elsewhere).
pub(super) fn latest<'a>(task: &'a Task, served: &Artifact) -> Option<&'a Artifact> {
    task.artifacts
        .iter()
        .rev()
        .filter(|a| a.purged_at.is_none())
        .find(|a| {
            a.source == served.source
                || (a.title == served.title && source_name(a) == source_name(served))
        })
}

/// What a document page shows, as one version: its URL (so the same bytes recorded by a later run count as
/// new) and bytes, plus the snapshot every recorded path of the task resolves to for its run (so a changed
/// stylesheet, script, image or data file counts as new even when the HTML is byte-identical). A page served
/// at the newest document gets exactly the version its event stream compares against, so it never loops.
pub(super) fn document_version(task: &Task, doc: &Artifact) -> String {
    let mut h = Sha256::new();
    h.update(artifact_url_path(doc).unwrap_or_default().as_bytes());
    h.update(b"\0");
    h.update(doc.sha256.as_bytes());
    let rels: std::collections::BTreeSet<String> = task.artifacts.iter().map(rel_path).collect();
    for rel in rels {
        if let Some(a) = resolve(task, &doc.run, &rel) {
            h.update(b"\n");
            h.update(rel.as_bytes());
            h.update(b"\0");
            h.update(a.path.as_bytes());
            h.update(a.sha256.as_bytes());
            h.update([u8::from(a.purged_at.is_some())]);
        }
    }
    h.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Percent-encodes one URL path segment (everything but unreserved characters).
pub(super) fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes one percent-encoded URL path segment (a literal `+` stays `+`); None for bad escapes or non-UTF-8.
pub(super) fn decode_segment(s: &str) -> Option<String> {
    decode(s, false)
}

/// Decodes one form-encoded query value (`+` is a space).
pub(super) fn decode_query(s: &str) -> Option<String> {
    decode(s, true)
}

fn decode(s: &str, plus_is_space: bool) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Escapes text for HTML element content and quoted attributes.
pub(super) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub(super) fn content_type(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "csv" => "text/csv; charset=utf-8",
        "txt" | "log" | "md" | "markdown" | "diff" | "patch" | "toml" | "yaml" | "yml" => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}

/// "14:32" in local time.
pub(super) fn hhmm(t: jiff::Timestamp) -> String {
    t.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%H:%M")
        .to_string()
}

const BAR_STYLE: &str = "all:initial;position:fixed;top:0;right:0;z-index:2147483647;\
font:12px/1.7 system-ui,sans-serif;background:rgba(31,35,40,.92);color:#f0f3f6;\
padding:2px 6px 2px 10px;border-bottom-left-radius:6px;box-shadow:0 1px 4px rgba(0,0,0,.3)";

/// The live header and its reload script. `watch` names what the event stream follows (an artifact URL path,
/// `index` or `t/<task>`), `version` what this page shows (sha or index fingerprint).
pub(super) fn live_header(
    token: &str,
    label: &str,
    index: &str,
    watch: &str,
    version: &str,
) -> String {
    format!(
        "<div id=\"corral-live\" style=\"{BAR_STYLE}\" data-watch=\"{watch}\" data-v=\"{version}\" \
data-token=\"{token}\"><span>{label}</span><span class=\"st\"></span> \
<a href=\"/{token}/{index}\" style=\"all:initial;color:#8cc4ff;cursor:pointer;font:inherit;margin-left:6px\">index</a>\
<button type=\"button\" title=\"hide\" style=\"all:initial;cursor:pointer;font:inherit;margin-left:8px;color:#f0f3f6\" \
onclick=\"this.parentNode.remove()\">&#215;</button></div>\
<script>(function(){{var b=document.getElementById('corral-live');if(!b||!window.EventSource)return;\
var q='watch='+encodeURIComponent(b.dataset.watch)+'&v='+encodeURIComponent(b.dataset.v);\
var s=new EventSource('/'+b.dataset.token+'/events?'+q),st=b.querySelector('.st');\
s.addEventListener('reload',function(e){{s.close();if(e.data===location.pathname)location.reload();else location.replace(e.data);}});\
s.onopen=function(){{st.textContent='';}};s.onerror=function(){{st.textContent=' \u{b7} offline';}};}})();</script>",
        label = esc(label),
        index = esc(index),
        watch = esc(watch),
        version = esc(version),
        token = esc(token),
    )
}

/// Inserts `header` right after the `<body ...>` tag, or appends it when the document has none.
pub(super) fn inject(html: &str, header: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let at = lower
        .match_indices("<body")
        .find(|(i, _)| {
            lower[i + 5..]
                .chars()
                .next()
                .is_some_and(|c| c == '>' || c.is_ascii_whitespace())
        })
        .and_then(|(i, _)| lower[i..].find('>').map(|j| i + j + 1));
    match at {
        Some(at) => format!("{}{header}{}", &html[..at], &html[at..]),
        None => format!("{html}{header}"),
    }
}

const PAGE_STYLE: &str = "body{font:15px/1.55 system-ui,sans-serif;max-width:980px;margin:2.5em auto;\
padding:0 1.2em;color:#1f2328;background:#fff}a{color:#0969da}code,pre{font:13px ui-monospace,monospace}\
pre{background:#f6f8fa;padding:.8em;overflow:auto;border-radius:6px}code{background:#f6f8fa;padding:0 .2em}\
pre code{padding:0}table{border-collapse:collapse}td,th{border:1px solid #d0d7de;padding:.3em .6em;text-align:left}\
blockquote{margin:0;padding:0 1em;color:#59636e;border-left:.25em solid #d0d7de}img{max-width:100%}\
.tier{font:11px ui-monospace,monospace;padding:0 .4em;border-radius:4px;background:#eef}\
.keep{background:#dafbe1}.transient{background:#f6f8fa;color:#59636e}td.dim{color:#59636e}";

pub(super) fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{PAGE_STYLE}</style></head>\
<body>{body}</body></html>",
        esc(title)
    )
}

/// The listed documents: the newest version of each HTML and Markdown source of one task or the workspace,
/// importance first (T-063 tiers).
pub(super) fn index_rows(ctx: &Ctx, task: Option<&str>) -> anyhow::Result<Vec<crate::ArtifactRow>> {
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<_> = crate::all_artifacts(ctx)?
        .into_iter()
        .filter(|r| task.is_none_or(|t| r.task_id == t))
        .filter(|r| r.artifact.purged_at.is_none() && is_document(&r.artifact))
        .filter(|r| seen.insert((r.task_id.clone(), r.artifact.source.clone())))
        .collect();
    rows.sort_by(|a, b| crate::cmp_importance(&a.artifact, &b.artifact));
    Ok(rows)
}

/// Changes whenever the listed set changes.
pub(super) fn fingerprint(rows: &[crate::ArtifactRow]) -> String {
    let mut h = Sha256::new();
    for r in rows {
        h.update(r.artifact.path.as_bytes());
        h.update(r.artifact.tier().as_str().as_bytes());
        h.update(b"\n");
    }
    h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(super) fn index_page(token: &str, task: Option<&str>, rows: &[crate::ArtifactRow]) -> String {
    let title = match task {
        Some(t) => format!("{t} documents"),
        None => "Workspace documents".to_string(),
    };
    let mut body = format!("<h1>{}</h1>", esc(&title));
    if task.is_some() {
        body.push_str(&format!("<p><a href=\"/{token}/\">all tasks</a></p>"));
    }
    if rows.is_empty() {
        body.push_str("<p>No HTML or Markdown artifacts yet.</p>");
    } else {
        body.push_str(
            "<table><tr><th>tier</th><th>task</th><th>run</th><th>kind</th><th>title</th><th>updated</th></tr>",
        );
        for r in rows {
            let a = &r.artifact;
            let Some(url) = artifact_url_path(a) else {
                continue;
            };
            let tier = a.tier().as_str();
            body.push_str(&format!(
                "<tr><td><span class=\"tier {tier}\">{tier}</span></td>\
<td><a href=\"/{token}/t/{tid}/\" title=\"{ttitle}\">{task}</a></td><td>{run}</td><td class=\"dim\">{kind}</td>\
<td><a href=\"/{token}/{url}\">{title}</a></td><td class=\"dim\" title=\"{full}\">{at}</td></tr>",
                tid = encode(&r.task_id),
                ttitle = esc(&r.task_title),
                task = esc(&r.task_id),
                run = esc(&a.run),
                kind = a.kind.as_str(),
                title = esc(&a.title),
                full = esc(&a.added.to_string()),
                at = hhmm(a.added),
            ));
        }
        body.push_str("</table>");
    }
    page(&title, &body)
}

/// Markdown rendered to an HTML fragment. Raw HTML in the source is shown as text, `javascript:` links are
/// dropped and YAML frontmatter is skipped.
pub(super) fn markdown_html(src: &str) -> String {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut out = String::new();
    let (mut in_head, mut in_meta, mut in_image) = (false, false, 0usize);
    for event in Parser::new_ext(src, opts) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => out.push_str(&format!("<{level}>")),
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::CodeBlock(CodeBlockKind::Fenced(lang)) if !lang.is_empty() => {
                    out.push_str(&format!("<pre><code class=\"language-{}\">", esc(&lang)));
                }
                Tag::CodeBlock(_) => out.push_str("<pre><code>"),
                Tag::List(Some(start)) => out.push_str(&format!("<ol start=\"{start}\">")),
                Tag::List(None) => out.push_str("<ul>"),
                Tag::Item => out.push_str("<li>"),
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::Strikethrough => out.push_str("<del>"),
                Tag::Link {
                    dest_url, title, ..
                } => out.push_str(&format!(
                    "<a href=\"{}\" title=\"{}\">",
                    esc(&safe_url(&dest_url)),
                    esc(&title)
                )),
                Tag::Image { dest_url, .. } => {
                    in_image += 1;
                    if in_image == 1 {
                        out.push_str(&format!(
                            "<img src=\"{}\" alt=\"",
                            esc(&safe_url(&dest_url))
                        ));
                    }
                }
                Tag::Table(_) => out.push_str("<table>"),
                Tag::TableHead => {
                    in_head = true;
                    out.push_str("<thead><tr>");
                }
                Tag::TableRow => out.push_str("<tr>"),
                Tag::TableCell => out.push_str(if in_head { "<th>" } else { "<td>" }),
                Tag::MetadataBlock(_) => in_meta = true,
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => out.push_str("</p>\n"),
                TagEnd::Heading(level) => out.push_str(&format!("</{level}>\n")),
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>\n"),
                TagEnd::CodeBlock => out.push_str("</code></pre>\n"),
                TagEnd::List(true) => out.push_str("</ol>\n"),
                TagEnd::List(false) => out.push_str("</ul>\n"),
                TagEnd::Item => out.push_str("</li>\n"),
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::Strikethrough => out.push_str("</del>"),
                TagEnd::Link => out.push_str("</a>"),
                TagEnd::Image => {
                    in_image = in_image.saturating_sub(1);
                    if in_image == 0 {
                        out.push_str("\">");
                    }
                }
                TagEnd::Table => out.push_str("</table>\n"),
                TagEnd::TableHead => {
                    in_head = false;
                    out.push_str("</tr></thead>\n");
                }
                TagEnd::TableRow => out.push_str("</tr>\n"),
                TagEnd::TableCell => out.push_str(if in_head { "</th>" } else { "</td>" }),
                TagEnd::MetadataBlock(_) => in_meta = false,
                _ => {}
            },
            Event::Text(t) if !in_meta => out.push_str(&esc(&t)),
            Event::Text(_) => {}
            Event::Code(t) if in_image > 0 => out.push_str(&esc(&t)),
            Event::Code(t) => out.push_str(&format!("<code>{}</code>", esc(&t))),
            Event::Html(t) | Event::InlineHtml(t) => out.push_str(&esc(&t)),
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                out.push_str(&format!("<code>{}</code>", esc(&t)));
            }
            Event::FootnoteReference(t) => out.push_str(&format!("[^{}]", esc(&t))),
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak if in_image > 0 => out.push(' '),
            Event::HardBreak => out.push_str("<br>"),
            Event::Rule => out.push_str("<hr>\n"),
            Event::TaskListMarker(done) => out.push_str(if done {
                "<input type=\"checkbox\" checked disabled> "
            } else {
                "<input type=\"checkbox\" disabled> "
            }),
        }
    }
    out
}

fn safe_url(url: &str) -> String {
    let scheme = url.trim_start().to_ascii_lowercase();
    if scheme.starts_with("javascript:")
        || scheme.starts_with("vbscript:")
        || scheme.starts_with("data:text/html")
    {
        "#".to_string()
    } else {
        url.to_string()
    }
}
