//! Browser preview sinks stage bytes privately until the completed snapshot can be launched.
use std::{fmt, future::Future, pin::Pin};

use nsh_core::prelude::Signal;

use crate::model::session::files_large::HtmlProgress;

pub(crate) type HtmlFinish = Pin<Box<dyn Future<Output = Result<(), String>>>>;

pub(crate) trait HtmlSink {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn finish(self: Box<Self>) -> HtmlFinish;
}

#[cfg(target_arch = "wasm32")]
pub(crate) type Output = Box<dyn HtmlSink>;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Output = Box<dyn HtmlSink + Send>;

pub struct OpenHtml {
    pub(crate) id: String,
    pub(crate) session: String,
    pub(crate) path: String,
    pub(crate) status: Signal<HtmlProgress>,
    pub(crate) output: Output,
}

impl fmt::Debug for OpenHtml {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenHtml")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
