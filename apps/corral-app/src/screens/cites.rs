//! Citations drawn where text cites a task or an artifact: inline, a Markdown link over each
//! citation that resolves (a press opens it; one that does not stays plain text), and under the
//! text a row per citation that the keyboard reaches as every other linked row
//! (`cite-<citation>`), saying "not found" for one that does not resolve. The cited tasks'
//! details are kept loaded while the text shows, so citations resolve as corrald answers.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_base::text_view::{Block, Inline};
use nsh_core::prelude::*;

use corral_types::references_in;

use crate::model::Form;
use crate::model::cite::{self, Cite, SCHEME};
use crate::state::AppState;
use crate::ui::term;

use super::task::keep::keep_details;
use super::task::parts::{link_row, mono_words};

/// Keeps the details that resolve `cites` loaded while the calling view shows.
pub(crate) fn keep_cited(app: AppState, cites: &[Cite]) {
    let targets = cite::targets(cites);
    let targets = computed(move || targets.clone());
    keep_details(app, targets, None);
    // A failed read is not retried by itself; a cited task's changed detail asks again.
    let app = untracked(|| app.scope());
    let cites = cites.to_vec();
    let watched = cites.clone();
    let versions = computed(move || app.feed.with(|feed| cite::versions(feed, &watched)));
    let seen = Rc::new(RefCell::new(None::<Vec<Option<String>>>));
    effect(move || {
        let now = versions.get();
        let was = seen.replace(Some(now.clone()));
        if was.is_some_and(|was| was != now) {
            untracked(|| {
                for target in app.feed.with(|feed| cite::failed_reads(feed, &cites)) {
                    app.refresh(target);
                }
            });
        }
    });
}

/// Every citation of `blocks` among `open` as a link to it; inside an existing link nothing
/// changes, and inline code that is exactly a citation becomes a link around the code.
pub(crate) fn link_blocks(blocks: &mut [Block], open: &[String]) {
    for block in blocks {
        match block {
            Block::Paragraph { content } | Block::Heading { content, .. } => {
                *content = link_inlines(std::mem::take(content), open);
            }
            Block::Blockquote { children, .. } => link_blocks(children, open),
            Block::List { items, .. } => {
                for item in items {
                    link_blocks(&mut item.children, open);
                }
            }
            Block::Table(table) => {
                for cell in table.head.iter_mut().chain(table.rows.iter_mut().flatten()) {
                    cell.content = link_inlines(std::mem::take(&mut cell.content), open);
                }
            }
            Block::Details {
                summary, children, ..
            } => {
                *summary = link_inlines(std::mem::take(summary), open);
                link_blocks(children, open);
            }
            _ => {}
        }
    }
}

fn link(spelled: &str, content: Vec<Inline>) -> Inline {
    Inline::Link {
        url: format!("{SCHEME}{spelled}"),
        title: None,
        content,
    }
}

fn link_inlines(inlines: Vec<Inline>, open: &[String]) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Text { text } => link_text(&text, open, &mut out),
            Inline::Code { code } if open.contains(&code) => {
                let spelled = code.clone();
                out.push(link(&spelled, vec![Inline::Code { code }]));
            }
            Inline::Strong { content } => out.push(Inline::Strong {
                content: link_inlines(content, open),
            }),
            Inline::Emphasis { content } => out.push(Inline::Emphasis {
                content: link_inlines(content, open),
            }),
            Inline::Strikethrough { content } => out.push(Inline::Strikethrough {
                content: link_inlines(content, open),
            }),
            Inline::Underline { content } => out.push(Inline::Underline {
                content: link_inlines(content, open),
            }),
            other => out.push(other),
        }
    }
    out
}

/// `text` as runs of words and links, each occurrence of an open citation a link. The grammar
/// lists a spelling once, so each pass takes the first citation of what is left.
fn link_text(text: &str, open: &[String], out: &mut Vec<Inline>) {
    let mut words = String::new();
    let mut rest = text;
    while let Some(found) = references_in(rest).into_iter().next() {
        let spelled = found.as_str();
        let start = spelled.as_ptr() as usize - rest.as_ptr() as usize;
        let end = start + spelled.len();
        words.push_str(&rest[..start]);
        if open.iter().any(|known| known == spelled) {
            if !words.is_empty() {
                out.push(Inline::text(std::mem::take(&mut words)));
            }
            out.push(link(spelled, vec![Inline::text(spelled.to_owned())]));
        } else {
            words.push_str(spelled);
        }
        rest = &rest[end..];
    }
    words.push_str(rest);
    if !words.is_empty() {
        out.push(Inline::text(words));
    }
}

/// The rows of the citations `text` holds, or nothing when it cites none.
pub(crate) fn cited(app: AppState, text: &str) -> Option<AnyElement> {
    let cites = cite::cites(text);
    (!cites.is_empty()).then(|| rows(app, cites))
}

/// A row per citation: its spelling, the title or why it opens nothing, its kind.
pub(crate) fn rows(app: AppState, cites: Vec<Cite>) -> AnyElement {
    keep_cited(app, &cites);
    let lines = computed(move || app.feed.with(|feed| cite::lines(feed, &cites)));
    let list = dynamic(move || {
        let rows: Vec<AnyElement> = lines
            .get()
            .into_iter()
            .map(|line| {
                let spelled = line.cite.text.clone();
                link_row(
                    app,
                    line.test_id,
                    line.label,
                    vec![mono_words(app, line.cite.text)],
                    line.title,
                    line.kind,
                    move || app.open_cite(&spelled),
                )
            })
            .collect();
        view().children(rows).into_element()
    });
    match app.form {
        Form::Terminal => view().pl(term::cells(2.0)).child(list).into_element(),
        Form::Desktop | Form::Phone => view().pt(px(4.0)).child(list).into_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A citation that does not resolve must stay plain text, and a repeated one must be a link
    /// at every place it is written, not only the first.
    #[test]
    fn only_open_citations_become_links_wherever_they_are_written() {
        let mut out = Vec::new();
        link_text(
            "see T-2:4#goal, T-9:9 and T-2:4#goal again",
            &["T-2:4#goal".to_owned()],
            &mut out,
        );
        let links: Vec<_> = out
            .iter()
            .filter_map(|inline| match inline {
                Inline::Link { url, .. } => Some(url.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(links, ["cite:T-2:4#goal", "cite:T-2:4#goal"]);
        assert!(out.iter().any(|inline| matches!(
            inline,
            Inline::Text { text } if text.contains("T-9:9")
        )));
    }
}
