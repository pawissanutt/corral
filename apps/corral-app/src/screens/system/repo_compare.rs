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
                                "Base {} · head {}. Comparing bindings and resolved spans, not relationships. Unavailable source is not proof of a change.",
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

/// Mark only differences that the two served documents and pinned spans demonstrate. An absent
/// excerpt can be a failed or unavailable read, never proof that source was removed.
pub(super) fn compare(head: &Diagram, base: &Diagram) -> (Touch, Touch) {
    let mut h = Touch::default();
    let mut b = Touch::default();
    for el in head.elements() {
        if head.id != base.id || base.element(&el.name).is_none() {
            h.added.push(el.name.clone());
            continue;
        }
        // Match durable anchor values, not evidence order, excerpts, or partial
        // result lists. A line shift alone is not a change to the bound content.
        let changed = head.evidence.iter().any(|(key, after)| {
            if key.element != el.name {
                return false;
            }
            let Some(before) = base.evidence.get(key) else {
                return true;
            };
            if let (Some(a), Some(b)) = (after, before)
                && let (Some(x), Some(y)) = (&a.span, &b.span)
            {
                if x.basis != y.basis || x.basis == corral_types::lenses::SpanBasis::MixedEntry {
                    return false;
                }
                return a.path != b.path
                    || x.digest
                        .as_ref()
                        .zip(y.digest.as_ref())
                        .is_some_and(|(a, b)| a != b);
            }
            false
        }) || base
            .evidence
            .keys()
            .any(|k| k.element == el.name && !head.evidence.contains_key(k));
        if changed {
            h.touched.push(el.name.clone());
            b.touched.push(el.name.clone());
        }
    }
    b.removed = base
        .elements()
        .filter(|e| head.id != base.id || head.element(&e.name).is_none())
        .map(|e| e.name.clone())
        .collect();
    (h, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_types::lenses::{BoundSpan, EvidenceAnchor, LensLocation, RecipeEvidence};

    #[test]
    fn compares_recipe_scoped_bindings_not_excerpt_coverage() {
        let mut base = crate::model::artifacts::mermaid::parse("flowchart LR\na --> b").unwrap();
        let id = base.nodes[0].name.clone();
        let key = RecipeEvidence {
            element: id.clone(),
            anchor: EvidenceAnchor::Path { path: "src".into() },
        };
        let mut loc = LensLocation {
            element: id.clone(),
            path: "src".into(),
            line: 1,
            span: Some(BoundSpan {
                binding: 0,
                basis: corral_types::lenses::SpanBasis::Anchor,
                end_line: 3,
                digest: Some("abc".into()),
            }),
        };
        base.evidence.insert(key.clone(), Some(loc.clone()));
        let mut head = base.clone();
        head.evidence.insert(key.clone(), None);
        assert!(compare(&head, &base).0.touched.is_empty());
        loc.span.as_mut().unwrap().digest = Some("xyz".into());
        loc.span.as_mut().unwrap().basis = corral_types::lenses::SpanBasis::DefinitionRange;
        head.evidence.insert(key.clone(), Some(loc.clone()));
        assert!(compare(&head, &base).0.touched.is_empty());
        loc.span.as_mut().unwrap().basis = corral_types::lenses::SpanBasis::Anchor;
        head.evidence.insert(key, Some(loc));
        assert_eq!(compare(&head, &base).1.touched, [id.clone()]);
        head.evidence.clear();
        assert_eq!(compare(&head, &base).0.touched, [id.clone()]);
        head.id.push_str("-replacement");
        let (h, b) = compare(&head, &base);
        assert!(h.added.contains(&id) && b.removed.contains(&id) && h.touched.is_empty());
    }
}
