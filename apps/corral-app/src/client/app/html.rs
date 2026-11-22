use crate::model::session::files_large::{HtmlProgress, HtmlRead, openable};

use super::super::html::OpenHtml;
use super::*;

pub(super) const TAG_PREFIX: &str = "html:";

pub(super) struct HtmlTransfer {
    request: OpenHtml,
    read: HtmlRead,
}

impl HtmlTransfer {
    fn tag(&self) -> String {
        format!("{TAG_PREFIX}{}", self.request.id)
    }

    fn target(&self) -> DetailRef {
        let (offset, revision) = self.read.cursor();
        DetailRef::SessionFileChunk {
            id: self.request.session.clone(),
            path: self.request.path.clone(),
            offset,
            revision,
        }
    }

    fn next(&self) -> Vec<Outgoing> {
        vec![out(
            DETAIL_GET,
            value(DetailGet {
                target: self.target(),
                page: None,
                etag: None,
            }),
            Some(self.tag()),
        )]
    }

    fn percent(&self) -> Option<u8> {
        percent(&self.request)
    }

    fn fail(&self, problem: String) {
        if self.percent().is_some() {
            self.request.status.try_set(HtmlProgress::Failed(problem));
        }
    }

    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn accepts(&self, tag: &str) -> bool {
        tag.strip_prefix(TAG_PREFIX) == Some(self.request.id.as_str()) && self.percent().is_some()
    }
}

impl AppState {
    pub(super) fn open_html(&mut self, feed: Signal<Feed>, request: OpenHtml) -> Vec<Outgoing> {
        if percent(&request).is_none() {
            return vec![];
        }
        let transfer = HtmlTransfer {
            request,
            read: HtmlRead::default(),
        };
        let problem = if !self.ready {
            Some("The daemon is offline.")
        } else if !feed.with_untracked(|feed| feed.capable(SESSION_HTML_V1)) {
            Some("This daemon does not serve standalone HTML snapshots.")
        } else if !openable(&transfer.request.path, 0) {
            Some("Only .html and .htm files can be opened in the browser.")
        } else if self
            .html
            .as_ref()
            .is_some_and(|old| old.percent().is_some())
        {
            Some("Another HTML snapshot is already loading.")
        } else {
            None
        };
        if let Some(problem) = problem {
            transfer.fail(problem.into());
            return vec![];
        }
        let outgoing = transfer.next();
        self.html = Some(transfer);
        outgoing
    }

    pub(super) fn cancel_html(&mut self, id: &str) {
        if self.html.as_ref().is_some_and(|open| open.request.id == id) {
            self.html = None;
        }
    }

    pub(super) fn interrupt_html(&mut self) {
        if let Some(open) = self.html.take() {
            open.fail("The daemon connection changed before the HTML snapshot finished.".into());
        }
    }

    pub(super) async fn html_reply(
        &mut self,
        tag: &str,
        params: Value,
        outcome: Result<Value, (wire::rpc::ErrorCode, String)>,
    ) -> Vec<Outgoing> {
        if !self
            .html
            .as_ref()
            .is_some_and(|open| tag.strip_prefix(TAG_PREFIX) == Some(open.request.id.as_str()))
        {
            return vec![];
        }
        let Some(mut open) = self.html.take() else {
            return vec![];
        };
        let Some(old_percent) = open.percent() else {
            return vec![];
        };
        let chunk = match outcome {
            Ok(reply) => get_chunk(params, reply, &open.target()),
            Err((wire::rpc::ErrorCode::Conflict, _)) => {
                Err("The HTML file changed while opening. Try opening it again.".into())
            }
            Err((_, problem)) => Err(problem),
        };
        let result =
            chunk
                .and_then(|chunk| open.read.accept(chunk))
                .and_then(|(bytes, percent)| {
                    open.request.output.append(&bytes)?;
                    Ok(percent)
                });
        match result {
            Ok(100) => {
                open.request.status.try_set(HtmlProgress::Launching);
                let status = open.request.status;
                let result = open.request.output.finish().await;
                status.try_set(match result {
                    Ok(()) => HtmlProgress::Idle,
                    Err(problem) => HtmlProgress::Failed(problem),
                });
                vec![]
            }
            Ok(percent) => {
                if percent != old_percent {
                    open.request.status.try_set(HtmlProgress::Reading {
                        id: open.request.id.clone(),
                        percent,
                    });
                }
                let outgoing = open.next();
                self.html = Some(open);
                outgoing
            }
            Err(problem) => {
                open.fail(problem);
                vec![]
            }
        }
    }
}

fn percent(request: &OpenHtml) -> Option<u8> {
    match request.status.try_get()? {
        HtmlProgress::Reading { id, percent } if id == request.id => Some(percent),
        _ => None,
    }
}

fn get_chunk(params: Value, result: Value, target: &DetailRef) -> Result<SessionFileChunk, String> {
    let request: DetailGet = decode(params)?;
    let reply: DetailReply = decode(result)?;
    if &request.target == target
        && request.page.is_none()
        && request.etag.is_none()
        && reply.next.is_none()
        && let Some(DetailBody::SessionFileChunk(chunk)) = reply.body
    {
        return Ok(chunk);
    }
    Err("Invalid HTML snapshot response from the daemon.".into())
}

#[cfg(test)]
#[path = "html_tests.rs"]
mod tests;
