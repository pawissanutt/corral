//! External-link dispatch and browser selection-gesture filtering for Markdown text views.

use crate::model::cite;

#[cfg(target_arch = "wasm32")]
#[derive(Clone, PartialEq)]
struct LinkSelection {
    anchor: Option<web_sys::Node>,
    anchor_offset: u32,
    focus: Option<web_sys::Node>,
    focus_offset: u32,
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy)]
struct LinkPointer {
    id: i32,
    x: i32,
    y: i32,
    moved: bool,
    selection_collapsed: bool,
    selection_changed: bool,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static LINK_SELECTION_TRACKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LINK_SELECTION_AT_POINTER_DOWN: std::cell::RefCell<Option<LinkSelection>> = const { std::cell::RefCell::new(None) };
    static LINK_POINTER: std::cell::Cell<Option<LinkPointer>> = const { std::cell::Cell::new(None) };
    static LINK_SELECTION_GESTURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(target_arch = "wasm32")]
fn web_selection() -> Option<LinkSelection> {
    let selection = web_sys::window()?.get_selection().ok().flatten()?;
    (!selection.is_collapsed()).then(|| LinkSelection {
        anchor: selection.anchor_node(),
        anchor_offset: selection.anchor_offset(),
        focus: selection.focus_node(),
        focus_offset: selection.focus_offset(),
    })
}

/// Capture browser pointer selection gestures because NSH reports short drags as presses.
#[cfg(target_arch = "wasm32")]
pub(super) fn install_link_selection_tracking() {
    use wasm_bindgen::{JsCast, closure::Closure};

    LINK_SELECTION_TRACKING.with(|installed| {
        if installed.replace(true) {
            return;
        }
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            installed.set(false);
            return;
        };
        let down =
            Closure::<dyn FnMut(web_sys::PointerEvent)>::new(|event: web_sys::PointerEvent| {
                LINK_SELECTION_AT_POINTER_DOWN
                    .with(|selection| *selection.borrow_mut() = web_selection());
                LINK_POINTER.with(|pointer| {
                    pointer.set(Some(LinkPointer {
                        id: event.pointer_id(),
                        x: event.client_x(),
                        y: event.client_y(),
                        moved: false,
                        selection_collapsed: false,
                        selection_changed: false,
                    }))
                });
            });
        if document
            .add_event_listener_with_callback_and_bool(
                "pointerdown",
                down.as_ref().unchecked_ref(),
                true,
            )
            .is_err()
        {
            installed.set(false);
            return;
        }
        down.forget();

        let moved =
            Closure::<dyn FnMut(web_sys::PointerEvent)>::new(|event: web_sys::PointerEvent| {
                LINK_POINTER.with(|tracked| {
                    tracked.set(tracked.get().map(|mut pointer| {
                        if pointer.id == event.pointer_id() {
                            // Capture runs before the browser's move default. Sampling here
                            // observes a mousedown collapse even when selectionchange is coalesced.
                            let current = web_selection();
                            if pointer_moved(
                                pointer.x,
                                pointer.y,
                                event.client_x(),
                                event.client_y(),
                            ) {
                                pointer.moved = true;
                            }
                            LINK_SELECTION_AT_POINTER_DOWN.with(|before| {
                                (pointer.selection_collapsed, pointer.selection_changed) =
                                    selection_change_state(
                                        before.borrow().as_ref(),
                                        current.as_ref(),
                                        pointer.selection_collapsed,
                                        pointer.selection_changed,
                                    );
                            });
                        }
                        pointer
                    }))
                });
            });
        if document
            .add_event_listener_with_callback_and_bool(
                "pointermove",
                moved.as_ref().unchecked_ref(),
                true,
            )
            .is_err()
        {
            installed.set(false);
            return;
        }
        moved.forget();

        let changed = Closure::<dyn FnMut()>::new(|| {
            let current = web_selection();
            LINK_POINTER.with(|tracked| {
                tracked.set(tracked.get().map(|mut pointer| {
                    LINK_SELECTION_AT_POINTER_DOWN.with(|before| {
                        (pointer.selection_collapsed, pointer.selection_changed) =
                            selection_change_state(
                                before.borrow().as_ref(),
                                current.as_ref(),
                                pointer.selection_collapsed,
                                pointer.selection_changed,
                            );
                    });
                    pointer
                }))
            });
        });
        if document
            .add_event_listener_with_callback("selectionchange", changed.as_ref().unchecked_ref())
            .is_err()
        {
            installed.set(false);
            return;
        }
        changed.forget();

        let up =
            Closure::<dyn FnMut(web_sys::PointerEvent)>::new(|event: web_sys::PointerEvent| {
                let current = web_selection();
                let selected = LINK_POINTER.with(|pointer| {
                    pointer.get().is_some_and(|pointer| {
                        let final_change = LINK_SELECTION_AT_POINTER_DOWN.with(|before| {
                            selection_changed_now(
                                before.borrow().as_ref(),
                                current.as_ref(),
                                pointer.selection_collapsed,
                            )
                        });
                        pointer.id == event.pointer_id()
                            && (pointer.moved
                                || pointer_moved(
                                    pointer.x,
                                    pointer.y,
                                    event.client_x(),
                                    event.client_y(),
                                ))
                            && (pointer.selection_changed || final_change)
                    })
                });
                LINK_SELECTION_GESTURE.with(|gesture| gesture.set(selected));
            });
        if document
            .add_event_listener_with_callback_and_bool(
                "pointerup",
                up.as_ref().unchecked_ref(),
                true,
            )
            .is_err()
        {
            installed.set(false);
            return;
        }
        up.forget();

        // NSH registered its window pointerup handler first, so this clears after its callback.
        let Some(window) = web_sys::window() else {
            installed.set(false);
            return;
        };
        let clear = Closure::<dyn FnMut()>::new(clear_link_selection_gesture);
        if window
            .add_event_listener_with_callback("pointerup", clear.as_ref().unchecked_ref())
            .is_err()
        {
            installed.set(false);
            return;
        }
        clear.forget();
        let cancel = Closure::<dyn FnMut()>::new(clear_link_selection_gesture);
        if window
            .add_event_listener_with_callback("pointercancel", cancel.as_ref().unchecked_ref())
            .is_ok()
        {
            cancel.forget();
        } else {
            installed.set(false);
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn clear_link_selection_gesture() {
    LINK_SELECTION_GESTURE.with(clear_selection_gesture);
    LINK_POINTER.with(|pointer| pointer.set(None));
    LINK_SELECTION_AT_POINTER_DOWN.with(|selection| *selection.borrow_mut() = None);
}

#[cfg(target_arch = "wasm32")]
pub(super) fn link_press_is_selection() -> bool {
    LINK_SELECTION_GESTURE.with(take_selection_gesture)
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn pointer_moved(x: i32, y: i32, next_x: i32, next_y: i32) -> bool {
    x != next_x || y != next_y
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn take_selection_gesture(gesture: &std::cell::Cell<bool>) -> bool {
    gesture.replace(false)
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn clear_selection_gesture(gesture: &std::cell::Cell<bool>) {
    gesture.set(false);
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn selection_change_state<T: PartialEq>(
    before: Option<&T>,
    current: Option<&T>,
    collapsed: bool,
    changed: bool,
) -> (bool, bool) {
    match current {
        None => (collapsed || before.is_some(), changed),
        Some(_) => (collapsed, changed || collapsed || before != current),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn selection_changed_now<T: PartialEq>(
    before: Option<&T>,
    current: Option<&T>,
    collapsed: bool,
) -> bool {
    current.is_some() && (collapsed || before != current)
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn install_link_selection_tracking() {}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn link_press_is_selection() -> bool {
    false
}

pub(super) fn activate_link(url: &str, mut cite: impl FnMut(&str), mut external: impl FnMut(&str)) {
    if let Some(spelled) = url.strip_prefix(cite::SCHEME) {
        cite(spelled);
    } else if url.split_once(':').is_some_and(|(scheme, rest)| {
        (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            && rest.starts_with("//")
    }) {
        external(url);
    }
}
