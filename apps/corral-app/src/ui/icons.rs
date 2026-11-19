//! The mock's line icons, shipped as SVG files under `assets/icons/` and drawn as tinted images.
//!
//! Each file draws its strokes in opaque black; the image's tint shows the given colour through
//! that ink on every platform, so one file serves both appearances and every surface. A terminal
//! shows no images, so terminal code never asks for one.

use nsh_core::prelude::*;

use crate::model::glance::StatIcon;
use crate::model::{Destination, PhoneTab};

const NOW: ImageAsset = asset!("icons/now.svg");
const CHANGES: ImageAsset = asset!("icons/changes.svg");
const PLANS: ImageAsset = asset!("icons/plans.svg");
const AGENTS: ImageAsset = asset!("icons/agents.svg");
const REVIEW: ImageAsset = asset!("icons/review.svg");
const TIMELINE: ImageAsset = asset!("icons/timeline.svg");
const USAGE: ImageAsset = asset!("icons/usage.svg");
const SETTINGS: ImageAsset = asset!("icons/settings.svg");
const ASK: ImageAsset = asset!("icons/chat.svg");
const SEARCH: ImageAsset = asset!("icons/search.svg");
const MORE: ImageAsset = asset!("icons/more.svg");
const BACK: ImageAsset = asset!("icons/back.svg");
const CHEVRON: ImageAsset = asset!("icons/chev.svg");
const CLOSE: ImageAsset = asset!("icons/close.svg");
const BRAND: ImageAsset = asset!("icons/brand.svg");
const ZOOM: ImageAsset = asset!("icons/zoom.svg");
const SPLIT: ImageAsset = asset!("icons/split.svg");
const TERM: ImageAsset = asset!("icons/term.svg");
const SEND: ImageAsset = asset!("icons/send.svg");
const LOCK: ImageAsset = asset!("icons/lock.svg");
const PHONE: ImageAsset = asset!("icons/phone.svg");
const QR: ImageAsset = asset!("icons/qr.svg");
const SIGNAL: ImageAsset = asset!("icons/signal.svg");
const BATTERY: ImageAsset = asset!("icons/batt.svg");
const SIDEBAR: ImageAsset = asset!("icons/sidebar.svg");
const PLUS: ImageAsset = asset!("icons/plus.svg");
const PROJECT: ImageAsset = asset!("icons/project.svg");
const PIN: ImageAsset = asset!("icons/pin.svg");
// The count glyphs share the rail marks' and actor dots' 12-unit shapes, so a stat reads as the
// mark it counts.
const NEEDS_YOU: ImageAsset = asset!("icons/actor-solid.svg");
const MOVING: ImageAsset = asset!("icons/mk-active.svg");
const STALE: ImageAsset = asset!("icons/mk-stale.svg");
const LANDED: ImageAsset = asset!("icons/mk-done.svg");
const FAILED: ImageAsset = asset!("icons/mk-fail.svg");
const QUEUED: ImageAsset = asset!("icons/queue.svg");
const PARKED: ImageAsset = asset!("icons/park.svg");
const HELD: ImageAsset = asset!("icons/actor-diamond.svg");
const EVENTS: ImageAsset = asset!("icons/delta.svg");
const FILES: ImageAsset = asset!("icons/note.svg");

/// The mock's sprite (`ui.js`, `i-*`). `Ask` is the sprite's `i-chat` speech bubble.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Now,
    Changes,
    Plans,
    Agents,
    Review,
    Timeline,
    Usage,
    Settings,
    Ask,
    Search,
    More,
    Back,
    Chevron,
    Close,
    Brand,
    /// A dock pane's maximise control.
    Zoom,
    /// A dock pane's split control.
    Split,
    /// A terminal view (Session's Terminal toggle).
    Term,
    /// The composer's Send arrow.
    Send,
    /// A lock: a confirmation on this device, an end-to-end encrypted path.
    Lock,
    /// A paired phone.
    Phone,
    /// The pairing QR code.
    Qr,
    /// Signal bars of a drawn phone status bar; filled, not stroked.
    Signal,
    /// The battery of a drawn phone status bar, 2:1 wide; drawn square it sits centred.
    Battery,
    /// A window with its sidebar: the desktop's control that shows or hides the sidebar labels.
    Sidebar,
    /// The phone composer's attachments menu.
    Plus,
    /// A folder: a configured project (its page, the composer's default project).
    Project,
    /// A push pin: the quick thread kept at the top of Now.
    Pin,
    /// A filled dot: waits on the owner (the sprite's `m-you`).
    NeedsYou,
    /// A half-filled ring: verifying, producing, executing (`m-now`).
    Moving,
    /// A dashed ring: a result of an older candidate (`m-stale`).
    Stale,
    /// A check (`m-done`).
    Landed,
    /// A cross (`m-fail`).
    Failed,
    /// Three dots (`i-queue`).
    Queued,
    /// Two bars (`i-park`).
    Parked,
    /// A diamond: held on a rule or a question.
    Held,
    /// A filled triangle: recorded changes (`i-delta`).
    Events,
    /// A page with lines (`i-note`).
    Files,
}

impl Icon {
    /// The destination's own icon.
    pub const fn destination(destination: Destination) -> Icon {
        match destination {
            Destination::Now => Icon::Now,
            Destination::Changes => Icon::Changes,
            Destination::Plans => Icon::Plans,
            Destination::Agents => Icon::Agents,
            Destination::Review => Icon::Review,
            Destination::Timeline => Icon::Timeline,
            Destination::Usage => Icon::Usage,
            Destination::Artifacts => Icon::Files,
            Destination::Settings => Icon::Settings,
        }
    }

    /// The phone tab's icon: its destination's, and the three dots for More.
    pub const fn phone_tab(tab: PhoneTab) -> Icon {
        match tab {
            PhoneTab::Now => Icon::Now,
            PhoneTab::Agents => Icon::Agents,
            PhoneTab::Plans => Icon::Plans,
            PhoneTab::More => Icon::More,
        }
    }

    /// The glyph of a count: the shape of the state it counts. Sessions are the Agents
    /// destination's terminal window.
    pub const fn stat(icon: StatIcon) -> Icon {
        match icon {
            StatIcon::NeedsYou => Icon::NeedsYou,
            StatIcon::Moving => Icon::Moving,
            StatIcon::Stale => Icon::Stale,
            StatIcon::Landed => Icon::Landed,
            StatIcon::Failed => Icon::Failed,
            StatIcon::Queued => Icon::Queued,
            StatIcon::Parked => Icon::Parked,
            StatIcon::Held => Icon::Held,
            StatIcon::Events => Icon::Events,
            StatIcon::Sessions => Icon::Agents,
            StatIcon::Files => Icon::Files,
        }
    }

    const fn asset(self) -> ImageAsset {
        match self {
            Icon::Now => NOW,
            Icon::Changes => CHANGES,
            Icon::Plans => PLANS,
            Icon::Agents => AGENTS,
            Icon::Review => REVIEW,
            Icon::Timeline => TIMELINE,
            Icon::Usage => USAGE,
            Icon::Settings => SETTINGS,
            Icon::Ask => ASK,
            Icon::Search => SEARCH,
            Icon::More => MORE,
            Icon::Back => BACK,
            Icon::Chevron => CHEVRON,
            Icon::Close => CLOSE,
            Icon::Brand => BRAND,
            Icon::Zoom => ZOOM,
            Icon::Split => SPLIT,
            Icon::Term => TERM,
            Icon::Send => SEND,
            Icon::Lock => LOCK,
            Icon::Phone => PHONE,
            Icon::Qr => QR,
            Icon::Signal => SIGNAL,
            Icon::Battery => BATTERY,
            Icon::Sidebar => SIDEBAR,
            Icon::Plus => PLUS,
            Icon::Project => PROJECT,
            Icon::Pin => PIN,
            Icon::NeedsYou => NEEDS_YOU,
            Icon::Moving => MOVING,
            Icon::Stale => STALE,
            Icon::Landed => LANDED,
            Icon::Failed => FAILED,
            Icon::Queued => QUEUED,
            Icon::Parked => PARKED,
            Icon::Held => HELD,
            Icon::Events => EVENTS,
            Icon::Files => FILES,
        }
    }
}

/// A square icon `size` points wide, drawn in `tint`, hidden from accessibility (the control around it
/// carries the label). Nothing at all on Form::Terminal callers: they simply do not call it.
pub fn icon(icon: Icon, size: f32, tint: AdaptiveColor) -> AnyElement {
    image(icon.asset())
        .size(px(size))
        .flex_shrink_0()
        .object_fit(ResizeMode::Contain)
        .tint_color(tint)
        .accessibility_elements_hidden(true)
        .into_element()
}
