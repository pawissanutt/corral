//! A run of settled calls folded into one row (`✓ read ×5 · recovery.rs, evaluate.rs, …  1.2s`):
//! the row in the tool grammar, pressing it shows the run's rows under it, each a tool row of
//! its own (`item-<n>-<k>`).

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::transcript::Group;
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::parts::pressable;
use super::tool::{line, tool};

pub(super) fn group(app: AppState, group: &Group, id: &str) -> AnyElement {
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
            view()
                .test_id(&format!("item-{id}"))
                .min_w(px(0.0))
                .child(tool(app, call, &id))
        });
        if app.form == Form::Terminal {
            view().pl(term::cells(2.0)).children(rows)
        } else {
            view()
                .px(px(8.0))
                .pb(px(6.0))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .children(rows)
        }
    });
    let block = view().min_w(px(0.0)).child(head).children(rows);
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
