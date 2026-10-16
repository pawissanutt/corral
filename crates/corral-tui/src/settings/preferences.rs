//! The Preferences section uses the same staged, layered save path as other settings.

use corral_core::{Accent, BOARD_KEYS, Config, Status};

use super::model::{Field, Kind, Row, number};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preference {
    Column(Status),
    Wip,
    Accent,
    Compact,
    DoneWindow,
    DoneMax,
    NotifyNeedsMe,
    NotifyReview,
    AutoDispatch,
}

fn toggle(field: Preference, label: impl Into<String>, key: &str, on: bool) -> Row {
    Row::new(
        Field::Preference(field),
        label,
        Kind::Toggle,
        if on { "on" } else { "off" }.into(),
    )
    .key(key)
}

pub(super) fn rows(cfg: &Config) -> Vec<Row> {
    let ui = &cfg.ui;
    let mut rows = vec![
        Row::new(
            Field::Preference(Preference::Accent),
            "Accent",
            Kind::Pick(Accent::ALL.iter().map(|a| Some(a.key().into())).collect()),
            ui.accent.key().into(),
        )
        .help("Accent for focus, links and keyboard hints; status colors keep their meaning.")
        .key("ui.accent"),
        toggle(
            Preference::Compact,
            "Compact cards",
            "ui.compact_cards",
            ui.compact_cards,
        )
        .help("Use one-line cards; off restores the density chosen in Board cards."),
        toggle(
            Preference::Wip,
            "Show WIP usage",
            "ui.show_wip",
            ui.show_wip,
        )
        .help("Display slot usage in the header and Running column; limits still apply."),
        toggle(
            Preference::NotifyNeedsMe,
            "Desktop: Needs me",
            "ui.notify_needs_me",
            ui.notify_needs_me,
        )
        .help("Opt-in desktop alerts when a task enters Needs me; separate from herdr alerts."),
        toggle(
            Preference::NotifyReview,
            "Desktop: Review",
            "ui.notify_review",
            ui.notify_review,
        )
        .help("Opt-in desktop alerts when a task enters Review."),
        toggle(
            Preference::AutoDispatch,
            "Auto-dispatch queue",
            "dispatch.paused",
            !cfg.dispatch.paused,
        )
        .help("Workspace only. Off pauses automatic admission; explicit dispatch still works."),
    ];
    for column in Status::COLUMNS {
        rows.push(
            toggle(
                Preference::Column(column),
                format!("Column: {}", column.label()),
                "ui.columns",
                ui.columns.contains(&column),
            )
            .help("Choose visible board columns; at least one must remain visible."),
        );
    }
    rows.extend([
        Row::new(
            Field::Preference(Preference::DoneWindow),
            "Done window",
            Kind::Text { optional: false },
            ui.done_window.clone(),
        )
        .help("Recent Done cards: positive age such as 24h, 7d or 2w; pinned cards stay.")
        .key("ui.done_window"),
        Row::new(
            Field::Preference(Preference::DoneMax),
            "Done card limit",
            Kind::Number { optional: false },
            ui.done_max.to_string(),
        )
        .help("Maximum recent Done cards, plus pinned cards. Older tasks remain in History.")
        .key("ui.done_max"),
    ]);
    for (action, default, label) in BOARD_KEYS {
        rows.push(
            Row::new(
                Field::KeyBinding(action.into()),
                format!("Key: {label}"),
                Kind::Text { optional: true },
                ui.keys.get(action).cloned().unwrap_or_default(),
            )
            .inherits(format!("{default} (default)"))
            .help("Board only. One printable ASCII character, not 0-9; empty restores default. No duplicates.")
            .key(format!("ui.keys.{action}")),
        );
    }
    rows
}

pub(super) fn set_value(cfg: &mut Config, field: &Field, text: &str) -> Result<(), String> {
    let ui = &mut cfg.ui;
    match field {
        Field::KeyBinding(action) => {
            if text.is_empty() {
                ui.keys.remove(action);
            } else {
                ui.keys.insert(action.clone(), text.into());
            }
        }
        Field::Preference(preference) => match preference {
            Preference::Column(column) => {
                ui.columns.retain(|s| s != column);
                if text == "on" {
                    ui.columns.push(*column);
                }
                ui.columns
                    .sort_by_key(|s| Status::COLUMNS.iter().position(|c| c == s));
            }
            Preference::Wip => ui.show_wip = text == "on",
            Preference::Accent => {
                ui.accent = Accent::ALL
                    .into_iter()
                    .find(|accent| accent.key() == text)
                    .ok_or_else(|| format!("{text:?} is not an accent"))?;
            }
            Preference::Compact => ui.compact_cards = text == "on",
            Preference::DoneWindow => ui.done_window = text.trim().into(),
            Preference::DoneMax => ui.done_max = number(text)?,
            Preference::NotifyNeedsMe => ui.notify_needs_me = text == "on",
            Preference::NotifyReview => ui.notify_review = text == "on",
            Preference::AutoDispatch => cfg.dispatch.paused = text != "on",
        },
        _ => return Err("not a UI preference".into()),
    }
    Ok(())
}
