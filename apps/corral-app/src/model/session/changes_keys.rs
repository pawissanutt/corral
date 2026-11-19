//! The Changes tab's rows in the session's action catalog and its own keys: `j`/`k` walk the
//! files, `n`/`p` the hunks (on into the next or previous file), `a` adds the current hunk to
//! the message, `c` steps to the next comparison the host can serve, `o` the next repository,
//! `f` the next author, `r` applies a newer patch or rereads, F3 (outside the web) goes to the
//! path filter. The keys act only while the tab shows and the composer does not have them.

use super::super::actions::{ActionGroup, ActionRow, row};
use super::super::keys::{KeyAction, KeyBinding, bind};
use super::super::{Action, Cx, Focus, Tab};
use super::{ChangesAction, ChangesTab, changes_tab, slug};

/// Whether the tab's own keys act: its tab shows and the composer does not have the keys.
fn keyed(cx: &Cx<'_>) -> bool {
    cx.ui.tab == Tab::Changes && cx.ui.focus != Focus::Composer
}

/// The next comparison the host can serve after the shown one.
fn next_comparison(tab: &ChangesTab) -> Option<Action> {
    let at = tab.comparisons.iter().position(|chip| chip.selected)?;
    let count = tab.comparisons.len();
    (1..count)
        .map(|step| &tab.comparisons[(at + step) % count])
        .find_map(|chip| chip.action.clone())
}

/// The item after the selected one, round to the first; none when it is the only one.
fn next_selected<T>(items: &[T], selected: impl Fn(&T) -> bool) -> Option<&T> {
    let at = items.iter().position(&selected)?;
    let next = items.get((at + 1) % items.len())?;
    (!selected(next)).then_some(next)
}

/// The tab's rows in the session's action catalog: every comparison the host offers (one it
/// cannot serve listed with its reason), and while a file is open its captures and Apply.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if !super::super::shown_tabs(cx).contains(&Tab::Changes) {
        return None;
    }
    // The diff is parsed only while the tab shows; otherwise the switch alone is listed.
    let (comparisons, selected) = if cx.ui.tab == Tab::Changes {
        let tab = changes_tab(cx);
        (tab.comparisons, tab.selected)
    } else {
        let target = super::shown_target(cx);
        (super::comparisons(cx, super::served(cx, &target)), None)
    };
    let mut rows: Vec<ActionRow> = comparisons
        .iter()
        .map(|chip| {
            let mut found = row(
                &format!("changes-compare-{}", slug(chip.value)),
                format!("Changes: compare {}", chip.label),
                Some("c"),
                // Listed with its reason when the host cannot serve it; the catalog runs no
                // unavailable row.
                Action::Changes(ChangesAction::Compare {
                    session: cx.id.clone(),
                    comparison: chip.value,
                }),
            );
            found.unavailable = chip.unavailable.clone();
            found
        })
        .collect();
    if let Some(diff) = &selected {
        if let Some(hunk) = diff.hunks.iter().find(|hunk| hunk.current) {
            rows.push(row(
                "changes-add-hunk",
                "Add hunk to message",
                Some("a"),
                hunk.capture.clone(),
            ));
        }
        rows.push(row(
            "changes-add-file",
            "Add file to message",
            None,
            diff.capture_file.clone(),
        ));
        if let Some(apply) = &diff.newer {
            rows.push(row(
                "changes-apply",
                "Apply new diff",
                Some("r"),
                apply.clone(),
            ));
        }
    }
    Some(ActionGroup {
        title: "Changes",
        rows,
    })
}

/// The tab's own keys.
pub fn keys() -> Vec<KeyBinding> {
    vec![
        bind("j", "j", "next file", KeyAction::Part),
        bind("k", "k", "previous file", KeyAction::Part),
        bind("n", "n", "next hunk", KeyAction::Part),
        bind("p", "p", "previous hunk", KeyAction::Part),
        bind("a", "a", "add hunk to message", KeyAction::Part),
        bind("c", "c", "next comparison", KeyAction::Part),
        bind("o", "o", "next repository", KeyAction::Part),
        bind("f", "f", "next author", KeyAction::Part),
        bind("r", "r", "reread · apply new diff", KeyAction::Part),
        KeyBinding {
            web: false,
            ..bind("f3", "F3", "filter paths", KeyAction::Part)
        },
    ]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    if !keyed(cx) {
        return None;
    }
    let tab = changes_tab(cx);
    let diff = tab.selected.as_ref();
    let first = || tab.files.first().map(|row| row.open.clone());
    match key {
        "j" => diff.map_or_else(first, |diff| diff.next_file.clone()),
        "k" => diff.and_then(|diff| diff.prev_file.clone()),
        "n" => diff.map_or_else(first, |diff| diff.next_hunk.clone()),
        "p" => diff.and_then(|diff| diff.prev_hunk.clone()),
        "a" => diff
            .and_then(|diff| diff.hunks.iter().find(|hunk| hunk.current))
            .map(|hunk| hunk.capture.clone()),
        "c" => next_comparison(&tab),
        "o" => next_selected(&tab.scopes, |chip| chip.selected).map(|chip| chip.action.clone()),
        "f" => next_selected(&tab.authors, |chip| chip.selected).map(|chip| chip.action.clone()),
        "r" => diff
            .and_then(|diff| diff.newer.clone())
            .or_else(|| tab.part.reread.clone()),
        "f3" => Some(Action::Changes(ChangesAction::FocusFilter {
            session: cx.id.clone(),
        })),
        _ => None,
    }
}
