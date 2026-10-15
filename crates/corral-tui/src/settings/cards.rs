//! The "Board cards" settings section: which fields board cards show, the density, the cost options and a live
//! preview of a sample card that follows the staged choices.

use corral_core::{BoardCfg, CardField, Config, Density, Status, status_key};

use super::card_preview;
use super::model::{Field, Kind, Row, opt, take};

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.into()
}

fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn rows(cfg: &Config) -> Vec<Row> {
    let card = &cfg.board.card;
    let mut out = vec![
        Row::new(
            Field::CardDensity,
            "Density",
            Kind::Pick(
                Density::ALL
                    .iter()
                    .map(|d| Some(d.label().to_string()))
                    .collect(),
            ),
            card.density.label().into(),
        )
        .help("compact: one line per card; comfortable: status lines; rich: adds a progress bar.")
        .key("board.card.density"),
        Row::new(
            Field::CardFields,
            "Field order",
            Kind::Text { optional: true },
            card.fields.join(", "),
        )
        .help("Highest priority first; at narrow widths the last fields are dropped first.")
        .key("board.card.fields"),
    ];
    for f in CardField::ALL {
        out.push(
            Row::new(
                Field::CardField(f),
                f.key(),
                Kind::Toggle,
                on_off(card.fields.iter().any(|k| k == f.key())),
            )
            .help(f.help()),
        );
    }
    out.push(
        Row::new(
            Field::CardCostWarn,
            "Cost warning (USD)",
            Kind::Text { optional: true },
            card.cost_warn.map(|w| w.to_string()).unwrap_or_default(),
        )
        .help("Cards at or above this cost show it in the warning color.")
        .inherits("no warning")
        .key("board.card.cost_warn"),
    );
    out.push(
        Row::new(
            Field::CardColumnCost,
            "Column cost sum",
            Kind::Toggle,
            on_off(card.column_cost),
        )
        .help("Show the summed cost of a column's cards in its header (when it fits)."),
    );
    for s in Status::COLUMNS {
        out.push(
            Row::new(
                Field::CardColumn(s),
                format!("{} fields", s.label()),
                Kind::Text { optional: true },
                card.columns
                    .get(status_key(s))
                    .map(|l| l.join(", "))
                    .unwrap_or_default(),
            )
            .help("Per-column override of the field order; empty = use the field order above.")
            .inherits("the field order above")
            .key(format!("board.card.columns.{}", status_key(s))),
        );
    }
    let mut all = card_preview::rows(cfg);
    all.extend(out);
    all
}

pub(super) fn set_value(cfg: &mut Config, field: &Field, text: &str) -> Result<(), String> {
    let card = &mut cfg.board.card;
    match field {
        Field::CardDensity => {
            card.density = Density::ALL
                .into_iter()
                .find(|d| d.label() == text)
                .ok_or_else(|| format!("{text:?} is not a density"))?;
        }
        Field::CardFields => card.fields = list(text),
        Field::CardField(f) => {
            card.fields.retain(|k| k != f.key());
            if text == "on" {
                card.fields.push(f.key().to_string());
            }
        }
        Field::CardCostWarn => {
            card.cost_warn = match opt(text) {
                None => None,
                Some(t) => Some(
                    t.parse::<f64>()
                        .map_err(|_| format!("{t:?} is not a number"))?,
                ),
            };
        }
        Field::CardColumnCost => card.column_cost = text == "on",
        Field::CardColumn(s) => {
            let names = list(text);
            if names.is_empty() {
                card.columns.remove(status_key(*s));
            } else {
                card.columns.insert(status_key(*s).to_string(), names);
            }
        }
        Field::CardPreview(_) => return Err("the preview is read-only".into()),
        _ => return Err("not a board card setting".into()),
    }
    Ok(())
}

pub(super) fn apply(base: &BoardCfg, draft: &BoardCfg, target: &mut BoardCfg) {
    take(
        &base.card.fields,
        &draft.card.fields,
        &mut target.card.fields,
    );
    take(
        &base.card.density,
        &draft.card.density,
        &mut target.card.density,
    );
    take(
        &base.card.columns,
        &draft.card.columns,
        &mut target.card.columns,
    );
    take(
        &base.card.cost_warn,
        &draft.card.cost_warn,
        &mut target.card.cost_warn,
    );
    take(
        &base.card.column_cost,
        &draft.card.column_cost,
        &mut target.card.column_cost,
    );
}
