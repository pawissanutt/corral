//! The mock's line icons, shipped as SVG files under `assets/icons/` and drawn as tinted images.
//!
//! Each file draws its strokes in opaque black; the image's tint shows the given colour through
//! that ink on every platform, so one file serves both appearances and every surface. A terminal
//! shows no images, so terminal code never asks for one.

use nsh_core::prelude::*;

use crate::model::{Destination, PhoneTab};

const INBOX: ImageAsset = asset!("icons/inbox.svg");
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Inbox,
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
}

impl Icon {
    /// The destination's own icon.
    pub const fn destination(destination: Destination) -> Icon {
        match destination {
            Destination::Inbox => Icon::Inbox,
            Destination::Now => Icon::Now,
            Destination::Changes => Icon::Changes,
            Destination::Plans => Icon::Plans,
            Destination::Agents => Icon::Agents,
            Destination::Review => Icon::Review,
            Destination::Timeline => Icon::Timeline,
            Destination::Usage => Icon::Usage,
            Destination::Settings => Icon::Settings,
        }
    }

    /// The phone tab's icon: its destination's, and the three dots for More.
    pub const fn phone_tab(tab: PhoneTab) -> Icon {
        match tab {
            PhoneTab::Inbox => Icon::Inbox,
            PhoneTab::Now => Icon::Now,
            PhoneTab::Agents => Icon::Agents,
            PhoneTab::Plans => Icon::Plans,
            PhoneTab::More => Icon::More,
        }
    }

    const fn asset(self) -> ImageAsset {
        match self {
            Icon::Inbox => INBOX,
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
