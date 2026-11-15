//! A device's authority in owner words, and what may change it from here. Every device, browser
//! or native, carries the same five flags; an administrator device edits another device's flags,
//! and a device may lower but never raise its own (corrald refuses self-escalation).

use corral_protocol::app::DeviceAuthority;

use crate::model::daemon::{Feed, Link};
use crate::model::devices::{DeviceLink, DeviceRow};

/// One authority flag, in the order the editor lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flag {
    Reply,
    Act,
    Assistant,
    AssistantAct,
    Admin,
}

impl Flag {
    pub const ALL: [Flag; 5] = [
        Flag::Reply,
        Flag::Act,
        Flag::Assistant,
        Flag::AssistantAct,
        Flag::Admin,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Flag::Reply => "Answer questions",
            Flag::Act => "Act on work",
            Flag::Assistant => "Ask assistants",
            Flag::AssistantAct => "Instruct assistants to act",
            Flag::Admin => "Administrator",
        }
    }

    /// What the flag lets the device do, in one line under its switch.
    pub fn detail(self) -> &'static str {
        match self {
            Flag::Reply => "answer Inbox questions and reply in threads",
            Flag::Act => "dispatch, accept, land and the other task and plan actions",
            Flag::Assistant => "ask assistants and file ideas in the chat",
            Flag::AssistantAct => "what is typed on it can make assistants act for you",
            Flag::Admin => {
                "change workspace settings and the mode, pair and revoke devices, and edit any device's authority"
            }
        }
    }

    /// Short form for a device row's summary.
    pub fn short(self) -> &'static str {
        match self {
            Flag::Reply => "answer",
            Flag::Act => "act",
            Flag::Assistant => "ask",
            Flag::AssistantAct => "instruct",
            Flag::Admin => "admin",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Flag::Reply => "reply",
            Flag::Act => "act",
            Flag::Assistant => "assistant",
            Flag::AssistantAct => "assistant-act",
            Flag::Admin => "admin",
        }
    }

    pub fn of(self, authority: &DeviceAuthority) -> bool {
        match self {
            Flag::Reply => authority.reply,
            Flag::Act => authority.act,
            Flag::Assistant => authority.assistant,
            Flag::AssistantAct => authority.assistant_act,
            Flag::Admin => authority.admin,
        }
    }

    pub fn set(self, authority: &mut DeviceAuthority, on: bool) {
        match self {
            Flag::Reply => authority.reply = on,
            Flag::Act => authority.act = on,
            Flag::Assistant => authority.assistant = on,
            Flag::AssistantAct => authority.assistant_act = on,
            Flag::Admin => authority.admin = on,
        }
    }
}

/// A device's flags in one line: what it may do, "read only", or "unknown" when corrald did
/// not report them.
pub fn summary(authority: Option<&DeviceAuthority>) -> String {
    let Some(authority) = authority else {
        return "authority unknown".into();
    };
    let on: Vec<&str> = Flag::ALL
        .into_iter()
        .filter(|flag| flag.of(authority))
        .map(Flag::short)
        .collect();
    if on.is_empty() {
        "read only".into()
    } else {
        on.join(" · ")
    }
}

/// Why this session may not edit `device`'s authority at all, when it may not. This device's own
/// row is editable (downward only, see [`may_turn_on`]).
pub fn refusal(feed: &Feed, device: &DeviceRow) -> Option<&'static str> {
    if device.revoked.is_some() {
        return Some("A revoked device has no authority to change.");
    }
    if device.authority.is_none() {
        return Some("corrald didn't report this device's authority, so it can't be edited here.");
    }
    if feed.link != Link::Up || feed.session.is_none() {
        return Some("Changing authority needs a live connection to corrald.");
    }
    if !own(feed, device) && !feed.session.as_ref().is_some_and(|s| s.authority.admin) {
        return Some(
            "Only an administrator device can change another device's authority; this one is not an administrator.",
        );
    }
    None
}

/// Whether `device` is the device this app runs on.
pub fn own(feed: &Feed, device: &DeviceRow) -> bool {
    matches!(&feed.device, DeviceLink::Enrolled { id, .. } if *id == device.id)
}

/// Why a flag that is off cannot be turned on in this draft, when it cannot: a device never
/// raises its own authority.
pub fn may_turn_on(draft: &Draft, flag: Flag) -> Result<(), &'static str> {
    if draft.own && !flag.of(&draft.from) {
        return Err(
            "A device can't raise its own authority; corrald refuses it. Turn this on from another administrator device.",
        );
    }
    Ok(())
}

/// An authority edit being written for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub device: String,
    pub name: String,
    pub from: DeviceAuthority,
    pub to: DeviceAuthority,
    /// The device this app runs on.
    pub own: bool,
}

impl Draft {
    /// The flags the draft turns on and off, in the editor's order.
    pub fn changes(&self) -> Vec<(Flag, bool)> {
        Flag::ALL
            .into_iter()
            .filter(|flag| flag.of(&self.from) != flag.of(&self.to))
            .map(|flag| (flag, flag.of(&self.to)))
            .collect()
    }

    /// Granting or keeping admin needs corrald's preview and this device's proof.
    pub fn needs_proof(&self) -> bool {
        self.to.admin
    }

    /// The question the confirmation asks: exactly what turns on and off, and what follows.
    pub fn question(&self) -> String {
        let changes = self.changes();
        let list = |on: bool| {
            changes
                .iter()
                .filter(|(_, to)| *to == on)
                .map(|(flag, _)| flag.label())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (on, off) = (list(true), list(false));
        let mut line = format!("Change {}'s authority?", self.name);
        if !on.is_empty() {
            line.push_str(&format!(" Turns on: {on}."));
        }
        if !off.is_empty() {
            line.push_str(&format!(" Turns off: {off}."));
        }
        if self.own && changes.iter().any(|&(flag, _)| flag == Flag::Admin) {
            line.push_str(
                " This device stops being an administrator and can't turn it back on itself.",
            );
        }
        if self.needs_proof() {
            line.push_str(
                " corrald then shows its exact preview, and you confirm it on this device; corrald checks this device's proof before applying it.",
            );
        }
        line
    }

    pub fn confirm_label(&self) -> &'static str {
        if self.needs_proof() {
            "Preview and confirm"
        } else {
            "Apply change"
        }
    }

    /// The command title corrald's confirmation names.
    pub fn title(&self) -> String {
        format!("Change {}'s authority", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(admin: bool) -> DeviceAuthority {
        DeviceAuthority {
            reply: true,
            act: true,
            assistant: true,
            assistant_act: false,
            admin,
        }
    }

    #[test]
    fn a_device_may_lower_but_never_raise_its_own_authority() {
        let draft = Draft {
            device: "d1".into(),
            name: "this browser".into(),
            from: authority(false),
            to: authority(false),
            own: true,
        };
        assert!(may_turn_on(&draft, Flag::Admin).is_err());
        assert!(may_turn_on(&draft, Flag::AssistantAct).is_err());
        assert!(may_turn_on(&draft, Flag::Act).is_ok());
        let other = Draft {
            own: false,
            ..draft
        };
        assert!(may_turn_on(&other, Flag::Admin).is_ok());
    }

    #[test]
    fn edits_that_leave_admin_on_need_the_proof_flow() {
        let mut draft = Draft {
            device: "d1".into(),
            name: "phone".into(),
            from: authority(true),
            to: authority(true),
            own: false,
        };
        Flag::Reply.set(&mut draft.to, false);
        assert_eq!(draft.changes(), vec![(Flag::Reply, false)]);
        assert!(draft.needs_proof());
        Flag::Admin.set(&mut draft.to, false);
        assert!(!draft.needs_proof());
    }
}
