//! A comparison pins the displayed file and both object IDs at the moment of the request.
use super::lenses::{Read, quiet, read_of};
use crate::model::system::lens::{Diagram, Touch, lens_request};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, button};
use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

#[derive(Clone, PartialEq)]
pub(super) struct Pins {
    pub file: String,
    pub head: String,
    pub base: String,
}

pub(super) fn bar(
    app: AppState,
    target: Memo<Option<DetailRef>>,
    pins: Signal<Option<Pins>>,
) -> AnyElement {
    let typed = signal(String::new());
    let field = copyable_input(app, None, move || typed.get())
        .placeholder("Base commit (full Git object ID)")
        .label("Base commit")
        .test_id("field-lens-base")
        .on_change(move |s: &str| typed.set(s.into()));
    let apply = move || {
        let base = typed.get_untracked().trim().to_ascii_lowercase();
        if !matches!(base.len(), 40 | 64) || !base.bytes().all(|b| b.is_ascii_hexdigit()) {
            app.toast("Enter the full Git commit ID for the base.");
            return;
        }
        if let Some(DetailRef::SystemLens {
            id, at: Some(head), ..
        }) = target.get_untracked()
        {
            pins.set(Some(Pins {
                file: id,
                head,
                base,
            }));
        }
    };
    view()
        .child(field)
        .child(button(
            app,
            "Compare at base",
            ButtonKind::Ghost,
            "btn-lens-compare",
            apply,
        ))
        .child(dynamic(move || {
            pins.get().map(|p| {
                untracked(|| {
                    view()
                        .child(quiet(
                            app,
                            format!(
                                "Base {} · head {}. Comparing bound source, not relationships.",
                                p.base, p.head
                            ),
                        ))
                        .child(button(
                            app,
                            "End comparison",
                            ButtonKind::Quiet,
                            "btn-lens-compare-end",
                            move || pins.set(None),
                        ))
                        .into_element()
                })
            })
        }))
        .into_element()
}

pub(super) fn read(app: AppState, target: &DetailRef) -> Read {
    app.feed.with(|f| read_of(f, target, &Touch::default()))
}
pub(super) fn target(project: &str, p: &Pins) -> DetailRef {
    lens_request(project, &p.file, Some(&p.base))
}

/// Mark only differences that the two served documents/excerpts demonstrate. An absent
/// excerpt can be a failed or unavailable read, never proof that source was removed.
pub(super) fn compare(head: &Diagram, base: &Diagram) -> (Touch, Touch) {
    let mut h = Touch::default();
    let mut b = Touch::default();
    for el in head.elements() {
        if base.element(&el.name).is_none() {
            h.added.push(el.name.clone());
            continue;
        }
        let lines = |d: &Diagram| {
            d.excerpts
                .iter()
                .filter(|e| e.element == el.name)
                .map(|e| {
                    (
                        &e.path,
                        e.line,
                        e.line
                            .checked_sub(e.first)
                            .and_then(|n| e.lines.get(n as usize)),
                    )
                })
                .map(|(p, l, s)| (p.clone(), l, s.cloned()))
                .collect::<Vec<_>>()
        };
        let (before, after) = (lines(base), lines(head));
        if !before.is_empty() && !after.is_empty() && before != after {
            h.touched.push(el.name.clone());
            b.touched.push(el.name.clone());
        }
    }
    b.removed = base
        .elements()
        .filter(|e| head.element(&e.name).is_none())
        .map(|e| e.name.clone())
        .collect();
    (h, b)
}
