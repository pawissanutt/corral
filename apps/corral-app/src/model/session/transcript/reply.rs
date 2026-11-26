//! A finished agent message's words as one paste-ready text, for its Copy reply action: the
//! Markdown it wrote, quotes and code included, without its time, tools or thinking.

use super::{Message, MessageItem, MessageKind, Transcript};

/// What Copy reply puts on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The message's written blocks as recorded, a blank line between them.
    pub text: String,
    /// Some block shows only its preview (its rest loading or unreadable): the text is not
    /// the whole reply, and the action must say so.
    pub partial: bool,
}

impl Reply {
    /// The action's words: a partial copy never passes for the whole reply.
    pub fn label(partial: bool) -> &'static str {
        if partial {
            "Copy shown part"
        } else {
            "Copy reply"
        }
    }

    /// The receipt once copied.
    pub fn receipt(&self) -> &'static str {
        if self.partial {
            "Copied the part shown; the rest of this reply is not loaded"
        } else {
            "Reply copied"
        }
    }
}

impl Transcript {
    /// The anchors of the messages a page edge may cut: the first while earlier pages exist,
    /// the last while newer ones do (`newer`). A record split across pages stays one message,
    /// so such a message may lack items another page holds and copies only as partial.
    pub fn cut(&self, newer: bool) -> [Option<String>; 2] {
        let older = self.older.is_some() || self.older_cursor.is_some();
        let anchor = |message: Option<&Message>| message.map(|m| m.anchor.clone());
        [
            older.then(|| anchor(self.messages.first())).flatten(),
            (newer || self.newer.is_some())
                .then(|| anchor(self.messages.last()))
                .flatten(),
        ]
    }
}

impl Message {
    /// The words of an agent's finished message, or nothing while it is being written or
    /// when it wrote none.
    pub fn reply(&self) -> Option<Reply> {
        if self.kind != MessageKind::Agent || self.live {
            return None;
        }
        let mut blocks = Vec::new();
        let mut partial = false;
        for item in &self.items {
            let MessageItem::Prose(prose) = item else {
                continue;
            };
            if prose.streaming {
                return None;
            }
            partial |= prose.more.size().is_some();
            let words = prose.markdown.trim_matches('\n');
            if !words.trim().is_empty() {
                blocks.push(words);
            }
        }
        (!blocks.is_empty()).then(|| Reply {
            text: blocks.join("\n\n"),
            partial,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{More, Prose, Thinking};
    use super::*;
    use crate::model::hue::Hue;
    use crate::model::session::Action;

    fn prose(markdown: &str, more: More) -> MessageItem {
        MessageItem::Prose(Prose {
            markdown: markdown.into(),
            more,
            streaming: false,
        })
    }

    fn message(items: Vec<MessageItem>) -> Message {
        Message {
            id: String::new(),
            anchor: "a".into(),
            kind: MessageKind::Agent,
            avatar: "C",
            hue: Hue::default(),
            head: Vec::new(),
            author: "chat".into(),
            detail: String::new(),
            tag: None,
            tag_facts: None,
            items,
            keys: Vec::new(),
            footer: None,
            live: false,
        }
    }

    /// A Code-mode handoff copied whole: the quoted paragraphs keep their `>` lines and the
    /// blank quote line between them, the paragraphs around them their line breaks, and the
    /// thinking before them stays out.
    #[test]
    fn copy_reply_keeps_a_quoted_multi_paragraph_handoff_and_leaves_thinking_out() {
        let handoff = "Paste this into Code mode:\n\n\
                       > Fix the selection in `markdown.rs`.\n>\n\
                       > Then run the gate and attach the log.\n\n\
                       That is all.";
        let mut reply = message(vec![
            MessageItem::Thinking(Thinking {
                text: "private reasoning".into(),
                open: true,
                hidden: String::new(),
                hidden_all: false,
                more: More::Whole,
                streaming: false,
                action: Action::Back,
            }),
            prose(&format!("\n{handoff}\n"), More::Whole),
            prose("Second block.", More::Whole),
        ]);
        assert_eq!(
            reply.reply(),
            Some(Reply {
                text: format!("{handoff}\n\nSecond block."),
                partial: false,
            })
        );
        reply.items.push(prose(
            "Only the preview",
            More::Preview {
                size: "12 KB".into(),
            },
        ));
        assert!(reply.reply().is_some_and(|reply| reply.partial));
        reply.live = true;
        assert_eq!(
            reply.reply(),
            None,
            "a reply still being written is not offered"
        );
    }

    /// A record split across pages stays one message whose other items are on a page not
    /// loaded: the oldest message while earlier pages exist and the newest while newer ones
    /// do are named as cut, so their copy is never passed off as whole.
    #[test]
    fn messages_at_a_page_edge_are_cut() {
        let mut page = super::super::transcript(
            &crate::model::Id::from("fixture"),
            &crate::model::session::Ui::default(),
            None,
            &crate::model::daemon::Feed::default(),
        );
        let mut second = message(Vec::new());
        second.anchor = "b".into();
        page.messages = vec![message(Vec::new()), second];
        assert_eq!(page.cut(false), [None, None]);
        page.older_cursor = Some("older".into());
        assert_eq!(page.cut(true), [Some("a".into()), Some("b".into())]);
    }
}
