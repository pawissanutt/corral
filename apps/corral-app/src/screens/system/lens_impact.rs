//! A lens a task's change touches, drawn in its Impact. At the head, with what the change
//! touched and added marked; at the base too, with what it removed (and touched there), when
//! it removed anything; only at the base when corrald has no such lens at the head. Each side
//! marks the names of its own revision, so a removed repeat never marks the head's survivor.
//! A head read that failed for any other reason, or a not-found one while the base is not
//! read, is said as such, never taken for a deletion.
//!
//! Test ids: "impact-lens", and those of `lens_canvas.rs` or the list form for each side.

use std::rc::Rc;

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use super::lens_canvas::OnPick;
use super::lenses::{Read, hued, inspector, read_of};
use crate::model::system::lens::{self, Touch};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::text::h3;

pub(crate) fn touched_lens(
    app: AppState,
    head: DetailRef,
    base: DetailRef,
    at_head: Touch,
    at_base: Touch,
) -> AnyElement {
    let form = app.form;
    let base_marks = !at_base.touched.is_empty() || !at_base.removed.is_empty();
    let head_read = computed(move || app.feed.with(|feed| read_of(feed, &head, &at_head)));
    let base_read = computed(move || app.feed.with(|feed| read_of(feed, &base, &at_base)));
    // A not-found head read is a deleted lens only once the same project answered at the
    // base: the project and the request are the same, so only the lens can be missing.
    let deleted = computed(move || {
        matches!(head_read.get(), Read::Missing) && matches!(base_read.get(), Read::Ready(..))
    });
    let parts = dynamic(move || {
        let deleted = deleted.get();
        untracked(|| {
            let mut parts = Vec::new();
            if !deleted {
                parts.push(marked_lens(app, head_read, None));
            }
            if deleted || base_marks {
                let heading = if deleted {
                    lens::AT_BASE_DELETED
                } else {
                    lens::AT_BASE
                };
                parts.push(marked_lens(app, base_read, Some(heading)));
            }
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 12.0 }))
                .children(parts)
                .into_element()
        })
    });
    view().test_id("impact-lens").child(parts).into_element()
}

/// One revision of a touched lens: what it says, the lens drawn with its marks, and the
/// inspector of the element picked on it.
pub(super) fn marked_lens(
    app: AppState,
    read: Memo<Read>,
    heading: Option<&'static str>,
) -> AnyElement {
    let form = app.form;
    let shown = computed(move || match read.get() {
        Read::Ready(d, _) => Some(*d),
        _ => None,
    });
    let chosen = signal(None::<String>);
    let picked = computed(move || chosen.get());
    let on_pick: OnPick = Rc::new(move |name: String| {
        chosen.update(|p| *p = (p.as_ref() != Some(&name)).then_some(name));
    });
    let said = dynamic(move || {
        let (hue, words) = match read.get() {
            Read::Loading => (Hue::Neutral, "Reading the lens…".to_owned()),
            Read::Missing => (Hue::Fail, lens::MISSING.to_owned()),
            Read::Failed(why) => (Hue::Fail, format!("The lens could not be read: {why}")),
            Read::Malformed(why) => (Hue::Fail, format!("Malformed: {}", why.join(" "))),
            Read::Ready(d, _) => (
                lens::status_hue(d.status),
                format!("{} · {}", d.provenance(), d.verdict()),
            ),
        };
        Some(untracked(|| hued(app, hue, words)))
    });
    let picture = super::repo_view::picture(app, shown, picked, on_pick);
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(heading.map(|h| h3(app, h, None)))
        .child(said)
        .child(picture)
        .child(inspector(app, shown, picked))
        .into_element()
}
