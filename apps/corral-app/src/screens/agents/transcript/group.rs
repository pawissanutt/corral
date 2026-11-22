//! A run of settled calls folded into one row (`✓ read ×5 · recovery.rs, evaluate.rs, …  1.2s`):
//! the row in the tool grammar, pressing it shows the run's rows under it, each a tool row of
//! its own (`item-<n>-<k>`). Drawn at a keyed spot, each opened row is placed by its call id,
//! in the group's rows, in the group, so the reader stays on the call they read while calls
//! join the run before and after it.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::transcript::Group;
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::parts::pressable;
use super::tool::{line, tool};
use super::{Spot, part, placed};

pub(super) fn group(app: AppState, group: &Group, id: &str, spot: Option<Spot<'_>>) -> AnyElement {
    let block_key = spot.map(|spot| format!("{}#group", spot.key));
    let rows_key = spot.map(|spot| format!("{}#rows", spot.key));
    let row = &group.row;
    let label = format!(
        "{} {} {}, a run of calls folded into one row",
        row.name, row.summary, row.outcome
    );
    let head = pressable(
        app,
        line(app, row, id),
        row.action.as_ref(),
        label,
        group.open,
        id,
    );
    let rows = group.open.then(|| {
        let rows = group.rows.iter().enumerate().map(|(k, call)| {
            let id = format!("{id}-{k}");
            let row = view()
                .test_id(&format!("item-{id}"))
                .min_w(px(0.0))
                .child(tool(app, call, &id))
                .into_element();
            match spot.zip(rows_key.as_deref()) {
                Some((spot, rows)) if call.fold.key.starts_with("call:") => {
                    placed(spot.scope, spot.anchor, &call.fold.key, Some(rows), row)
                }
                _ => row,
            }
        });
        let rows = if app.form == Form::Terminal {
            view().pl(term::cells(2.0)).children(rows)
        } else {
            view()
                .px(px(8.0))
                .pb(px(6.0))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .children(rows)
        };
        match spot.zip(rows_key.as_deref()) {
            Some((spot, key)) => part(spot.scope, spot.anchor, key, block_key.as_deref(), rows),
            None => rows,
        }
    });
    let block = view().min_w(px(0.0)).child(head).children(rows);
    // Placed in the group's item, under the block's top margin.
    let block = match spot.zip(block_key.as_deref()) {
        Some((spot, key)) => part(spot.scope, spot.anchor, key, Some(spot.key), block),
        None => block,
    };
    if app.form == Form::Terminal {
        return block.into_element();
    }
    block
        .mt(px(4.0))
        .mb(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .overflow_hidden()
        .into_element()
}
