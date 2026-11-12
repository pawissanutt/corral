//! Fixture content for Settings.

use super::{Device, Notification, NotificationKind, Profile, QueueRule, Step};
use crate::model::effect::Conn;

/// Paired devices in the snapshot.
pub const DEVICES: [Device; 4] = [
    Device {
        name: "pawis-desktop",
        platform: "Linux · desktop app",
        seen: "now",
        reach: "direct · loopback",
        this_device: true,
        may_instruct: "yes",
    },
    Device {
        name: "pawis-iphone",
        platform: "iOS 19 · app",
        seen: "14m ago",
        reach: "relay · e2e",
        this_device: false,
        may_instruct: "yes",
    },
    Device {
        name: "pawis-laptop",
        platform: "Firefox · web client",
        seen: "2d ago",
        reach: "direct · tailscale",
        this_device: false,
        may_instruct: "ask and file only",
    },
    Device {
        name: "terminal",
        platform: "corral app --tui · terminal target",
        seen: "5h ago",
        reach: "direct · loopback",
        this_device: false,
        may_instruct: "yes",
    },
];
/// Pairing instructions in order.
pub const STEPS: [Step; 4] = [
    Step {
        title: "Show the QR on a trusted surface",
        detail: "Here, or run corral device pair in a terminal on the daemon host.",
    },
    Step {
        title: "Scan it on the new device",
        detail: "The QR carries the daemon public key, reach hints and a one-time token (expires in 5 min).",
    },
    Step {
        title: "Compare the code on both screens",
        detail: "Both sides derive it from the key exchange; a mismatch means someone is in the middle.",
    },
    Step {
        title: "Confirm here",
        detail: "Only a trusted surface can confirm. The new device then appears below and can be revoked any time.",
    },
];
/// Illustrative key exchange payload.
pub const PAYLOAD: [(&str, &str); 3] = [
    ("daemon key", "ed25519 · 7F3A 91C2 · 0E5B D4A8"),
    (
        "reach hints",
        "loopback · lan 192.168.1.20:7419 · tailscale workstation.tail · relay relay.pawis.dev",
    ),
    ("token", "one-time · expires 22:17"),
];
/// Notification categories and their initial switches.
pub const NOTIFICATIONS: [Notification; 7] = [
    Notification {
        kind: NotificationKind::NeedsYou,
        label: "Needs you",
        detail: "question, owner decision, assistant prompts",
        default_on: true,
    },
    Notification {
        kind: NotificationKind::Plan,
        label: "Plan awaiting acceptance",
        detail: "reviewed plans",
        default_on: true,
    },
    Notification {
        kind: NotificationKind::Stuck,
        label: "Stuck",
        detail: "no progress for 45 min",
        default_on: true,
    },
    Notification {
        kind: NotificationKind::Escalated,
        label: "Escalated",
        detail: "PM escalated to you",
        default_on: true,
    },
    Notification {
        kind: NotificationKind::Daemon,
        label: "Daemon unhealthy",
        detail: "degraded > 5 min, or down",
        default_on: true,
    },
    Notification {
        kind: NotificationKind::Assistant,
        label: "Assistant answered",
        detail: "an ask finished while you were away",
        default_on: false,
    },
    Notification {
        kind: NotificationKind::Landed,
        label: "Landed",
        detail: "each landing",
        default_on: false,
    },
];
/// Rules for actions without a daemon connection.
pub const QUEUE_RULES: [QueueRule; 5] = [
    QueueRule {
        action: "Message to the assistant or PM",
        queued: true,
        reason: "Delivered once on reconnect; if more than 15 min late it can be answered and filed but cannot instruct",
    },
    QueueRule {
        action: "Answer question Q at revision N",
        queued: true,
        reason: "Carries rev N; on reconnect corrald applies it only if Q is still at rev N, otherwise it returns to you",
    },
    QueueRule {
        action: "Accept or reject a plan",
        queued: false,
        reason: "Needs a live revision check and a fresh confirm",
    },
    QueueRule {
        action: "Approve review, land, grants, autopilot, unstick",
        queued: false,
        reason: "High-risk: fresh confirm bound to a live preview",
    },
    QueueRule {
        action: "Pair or revoke a device",
        queued: false,
        reason: "Needs the daemon to issue or retire keys",
    },
];
/// Selectable assistant profiles.
pub const PROFILES: [Profile; 4] = [
    Profile {
        name: "pi-sol xhigh",
        model: "gpt-6.1-sol · xhigh",
        note: "assistant profile · default",
    },
    Profile {
        name: "pi-terra high",
        model: "gpt-5.6-terra · high",
        note: "",
    },
    Profile {
        name: "pi-luna",
        model: "gpt-6-luna · low",
        note: "",
    },
    Profile {
        name: "pi-opus high",
        model: "claude-opus-4-5 · high",
        note: "",
    },
];
/// Rows in the current connection inspector.
pub fn details(conn: Conn) -> &'static [(&'static str, &'static str)] {
    match conn {
        Conn::Healthy => &[
            ("corrald", "healthy · up 6d 3h · rev 418 207"),
            (
                "transport",
                "Unix socket /run/user/1000/corrald.sock · direct · loopback",
            ),
            (
                "ssh",
                "an SSH bridge (ssh workstation corral daemon connect) also shows as direct · loopback, marked via ssh here",
            ),
            (
                "herdr-core",
                "ok · 0.1 · 6 panes supervised · live terminal streams",
            ),
            (
                "herdr-tui",
                "ok · legacy · 1 run (T-854) · terminal is a refreshed snapshot",
            ),
            ("PM", "live · generation 14"),
            ("stores", "receipts 9 812 / 16 384 · rollover at 90%"),
        ],
        Conn::Degraded => &[
            ("corrald", "degraded · sync stalled 4m · last pass 22:08"),
            (
                "other reasons",
                "lock dispatch held 95s by pid N · unsupervised · relay unreachable",
            ),
            ("transport", "direct · loopback"),
            ("herdr-core", "ok"),
            ("herdr-tui", "ok"),
            ("PM", "relaunching · generation 15 starting"),
        ],
        Conn::Down => &[
            (
                "corrald",
                "down since 22:09 · exit 101 · service manager restarting (2/5)",
            ),
            ("transport", "socket refused · direct · loopback"),
            ("herdr-core", "unreachable (supervised by corrald)"),
            ("herdr-tui", "unreachable"),
            ("PM", "absent"),
            (
                "board",
                "last projection rev 418 199 at 22:08 · marked stale",
            ),
        ],
        Conn::Relay => &[
            ("corrald", "healthy"),
            (
                "transport",
                "relay · e2e · relay.pawis.dev · X25519 session, rotated 21:40",
            ),
            (
                "relay sees",
                "packet sizes and timing; never content, ids or titles",
            ),
            ("push", "content-free wake; text built on the phone"),
            ("latency", "180 ms round trip"),
        ],
        Conn::Offline => &[
            ("corrald", "unknown · last healthy 22:08"),
            ("transport", "offline · last sync 22:08"),
            (
                "local queue",
                "1 answer with a checkable expectation · 1 chat message",
            ),
            ("refused", "plan accept, approvals, grants, pairing"),
            ("board", "read-only · rev 418 199"),
        ],
    }
}
