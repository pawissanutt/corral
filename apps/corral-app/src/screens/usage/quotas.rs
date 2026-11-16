//! Where a provider's quota facts come from (RC-11): one quiet line per observation, the card
//! header's first (its amounts are the card's), every other with its own amounts. An exhausted
//! observation says so in the failure ink; a partial one says so under its line in the held ink.
//! Nothing when corral holds no observation: the card's amounts line already says so.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{ProviderView, QUOTA_TITLE, QuotaLine};
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::costs::qualifier;

/// The provider's quota sources under its figures, with a hairline above on desktop and phone.
/// test_id "usage-quotas-<provider id>".
pub fn quotas(app: AppState, item: &ProviderView) -> Option<AnyElement> {
    if item.quotas.is_empty() {
        return None;
    }
    let terminal = app.form == Form::Terminal;
    let lines = item
        .quotas
        .iter()
        .map(|quota| line(app, quota))
        .collect::<Vec<_>>();
    Some(
        view()
            .test_id(&format!("usage-quotas-{}", item.id))
            .role(AccessibilityRole::Group)
            .label(format!("{QUOTA_TITLE} · {}", item.label))
            .when(!terminal, |v| {
                v.mt(px(12.0))
                    .pt(px(10.0))
                    .gap(px(4.0))
                    .border_t(px(1.0))
                    .border_color(color(Token::Rule))
            })
            .children(lines)
            .into_element(),
    )
}

/// One observation: its line, then its coverage when partial.
fn line(app: AppState, quota: &QuotaLine) -> AnyElement {
    let ink = if quota.exhausted {
        Token::Fail
    } else {
        Token::Ink3
    };
    let words = text(quota.text.clone()).text_color(color(ink));
    let words = match app.form {
        Form::Terminal => words.number_of_lines(1u32).into_element(),
        Form::Desktop => words
            .font_family(FONT_TEXT)
            .text_size(px(11.5))
            .line_height(px(16.0))
            .into_element(),
        Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .line_height(px(18.0))
            .into_element(),
    };
    view()
        .child(if app.form == Form::Terminal {
            view()
                .flex_row()
                .h(term::rows(1.0))
                .child(words)
                .into_element()
        } else {
            words
        })
        .children(
            quota
                .completeness
                .as_deref()
                .map(|line| qualifier(app, line)),
        )
        .into_element()
}
