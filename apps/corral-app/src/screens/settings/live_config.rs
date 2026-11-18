//! Settings › Configuration: `corral.toml` as corrald last read it, read-only. Each part under
//! its caption: the automation switches, defaults and artifacts as a facts table; projects,
//! profiles, routing rules and harnesses as records parted by hairlines, each with its facts.

use nsh_core::prelude::*;

use super::live_sections::{facts, snapshot_section};
use super::parts::{Record, cell, records, space};
use crate::model::Hue;
use crate::model::settings::config::{self, Body, Group, Item};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::notice::notice;
use crate::ui::kit::text::h3;
use crate::ui::tokens::Token;

pub fn page(app: AppState) -> AnyElement {
    snapshot_section(app, "Configuration", |app, settings| {
        let mut parts = vec![notice(
            app,
            Hue::Neutral,
            &vec![span(config::READ_ONLY)],
            None,
        )];
        for group in config::groups(&settings.config) {
            parts.push(group_part(app, group));
        }
        parts
    })
}

fn group_part(app: AppState, group: Group) -> AnyElement {
    let mut parts = vec![h3(app, group.title, None)];
    if let Some(note) = group.note {
        parts.push(cell(app, note, Token::Ink3, false).into_element());
        parts.push(space(app, 6.0));
    }
    parts.push(match group.body {
        Body::Facts(rows) => facts(app, rows),
        Body::Items { items, .. } if !items.is_empty() => records(
            items
                .into_iter()
                .map(|Item { name, facts: rows }| Record {
                    lead: cell(app, name, Token::Ink, true).into_element(),
                    end: None,
                    under: vec![facts(app, rows)],
                })
                .collect(),
        ),
        Body::Items { empty, .. } => cell(app, empty, Token::Ink3, false).into_element(),
    });
    view().min_w(px(0.0)).children(parts).into_element()
}
