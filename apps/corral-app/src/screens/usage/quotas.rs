//! A provider's quota observations (RC-11): each quota corral holds evidence of, as its labelled
//! facts in the model's order (profile, source, scope, window, used, limit, headroom, resets,
//! observed, exhausted). A fact corral does not hold reads "unknown" in the quiet ink rather
//! than being left out, so an absent limit is never mistaken for none; a quota whose coverage is
//! partial says so under its facts in the held ink.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{ProviderView, QUOTA_TITLE, QuotaView, UNKNOWN};
use crate::state::AppState;
use crate::ui::kit::table::kv;
use crate::ui::kit::text::{dim, plain};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::costs::qualifier;

/// The provider's quotas under its figures; nothing when corral recorded none. test_id
/// "usage-quotas-<provider id>".
pub fn quotas(app: AppState, item: &ProviderView) -> Option<AnyElement> {
    if item.quotas.is_empty() {
        return None;
    }
    let terminal = app.form == Form::Terminal;
    let blocks = item
        .quotas
        .iter()
        .enumerate()
        .map(|(at, quota)| block(app, quota, at > 0))
        .collect::<Vec<_>>();
    Some(
        view()
            .test_id(&format!("usage-quotas-{}", item.id))
            .role(AccessibilityRole::Group)
            .label(format!("{QUOTA_TITLE} · {}", item.label))
            .when(!terminal, |v| v.mt(px(10.0)).gap(px(8.0)))
            .child(dim(app, QUOTA_TITLE).font_weight(FontWeight::SEMIBOLD))
            .children(blocks)
            .into_element(),
    )
}

/// One quota: its facts, then its coverage; a hairline (a blank row in a terminal) before every
/// quota but the first.
fn block(app: AppState, quota: &QuotaView, ruled: bool) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let rows = quota
        .facts()
        .into_iter()
        .map(|(label, value)| (label.to_owned(), fact(app, value)))
        .collect();
    view()
        .when(ruled && terminal, |v| v.child(term::blank()))
        .when(ruled && !terminal, |v| {
            v.pt(px(8.0))
                .border_t(px(1.0))
                .border_color(color(Token::Rule))
        })
        .when(!terminal, |v| v.gap(px(4.0)))
        .child(kv(app, rows))
        .children(
            quota
                .completeness
                .as_deref()
                .map(|line| qualifier(app, line)),
        )
        .into_element()
}

fn fact(app: AppState, value: &str) -> AnyElement {
    plain(app, value.to_owned())
        .text_color(color(if value == UNKNOWN {
            Token::Ink3
        } else {
            Token::Ink
        }))
        .into_element()
}
