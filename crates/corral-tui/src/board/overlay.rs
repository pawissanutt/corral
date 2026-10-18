use super::drop::ConfirmDrop;
use crate::View;
use crate::overlays::{CaptureView, DispatchSheet, HelpView};
use crate::shelf::panel::ShelfPanel;

pub(crate) enum Overlay {
    Capture(CaptureView),
    Dispatch(Box<DispatchSheet>),
    Help(HelpView),
    Shelf(ShelfPanel),
    Drop(ConfirmDrop),
}

impl Overlay {
    pub(super) fn view(&mut self) -> &mut dyn View {
        match self {
            Self::Capture(v) => v,
            Self::Dispatch(v) => &mut **v,
            Self::Help(v) => v,
            Self::Shelf(v) => v,
            Self::Drop(v) => v,
        }
    }

    pub(super) fn view_ref(&self) -> &dyn View {
        match self {
            Self::Capture(v) => v,
            Self::Dispatch(v) => &**v,
            Self::Help(v) => v,
            Self::Shelf(v) => v,
            Self::Drop(v) => v,
        }
    }
}
