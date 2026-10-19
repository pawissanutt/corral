use super::DetailView;
use super::{ACCEPTANCE, COMMENTS, PLAN, TIMELINE};

impl DetailView {
    pub(super) fn detail_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.overview {
            return self.overview_hints();
        }
        if self.timeline.overlay_open() {
            return vec![("j/k", "scroll"), ("pgup/pgdn", "scroll"), ("esc", "close")];
        }
        if self.routing.open {
            return self.routing.hints();
        }
        if self.comment.is_some() {
            return vec![
                ("⏎", "save comment"),
                ("⇥", "send option"),
                ("esc", "cancel"),
            ];
        }
        if self.preview.is_some() {
            return vec![
                ("j/k", "scroll"),
                ("⏎", "open externally"),
                ("esc", "close"),
            ];
        }
        if self.confirm.is_some() {
            return vec![("⏎", "unstick"), ("esc", "cancel")];
        }
        if self.plan.sheet_open() {
            return self.plan.hints(true).unwrap_or_default();
        }
        let mut h = vec![("0", "overview"), ("L", "hide/show live"), ("r", "route")];
        h.extend(match self.plan.enabled {
            true => vec![("1-8", "tabs"), ("F1-F4", "stages"), ("F5", "plan")],
            false => vec![("1-7", "tabs"), ("F1-F4", "stages")],
        });
        match self.tab {
            1 => h.extend([("j/k", "run"), ("pgup/pgdn", "scroll")]),
            2 => h.extend([
                ("j/k", "select"),
                ("⏎", "preview"),
                ("n", "next important"),
                ("v", "reviewed"),
                ("f", "raw fold"),
            ]),
            ACCEPTANCE => h.extend([("j/k", "record"), ("⏎", "preview"), ("pgup/pgdn", "scroll")]),
            COMMENTS => h.extend([("c", "add comment"), ("j/k", "scroll")]),
            TIMELINE => h.extend([("j/k", "event"), ("⏎", "raw event"), ("a", "all events")]),
            PLAN => h.extend(self.plan.hints(true).unwrap_or_default()),
            _ => h.push(("j/k", "scroll")),
        }
        if self.stuck().is_some() {
            h.push(("u", "unstick"));
        }
        if self.task.parent.is_some() {
            h.push(("P", "parent"));
        }
        let pinned = self.pins.contains(&self.pin_target());
        if self.tab == PLAN {
            h.extend([("*", if pinned { "unpin" } else { "pin" }), ("esc", "back")]);
        } else {
            if self.show_live {
                h.push(("[/]", "live tail"));
            }
            h.push(("*", if pinned { "unpin" } else { "pin" }));
            if self.tab != TIMELINE {
                h.push(("a", "jump"));
            }
            h.push(("esc", "back"));
        }
        h
    }
}
