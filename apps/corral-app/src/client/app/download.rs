//! An artifact's download: its own bounded transfer reading `DetailRef::Artifact` a page at a
//! time and handing each page's original bytes to the sink, never a retained detail. Only a read
//! that ends where the catalog says, with the catalog's SHA-256, is put in place.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

use crate::model::artifacts::download::Saving;

use super::super::download::SaveArtifact;
use super::*;

pub(super) const TAG_PREFIX: &str = "save:";

pub(super) struct SaveTransfer {
    request: SaveArtifact,
    /// The continuation of the page last read; none before the first.
    next: Option<String>,
    /// The first page's identity, which every later page must repeat.
    first: Option<(String, String, String)>,
    received: u64,
    hash: Sha256,
}

impl SaveTransfer {
    fn tag(&self) -> String {
        format!("{TAG_PREFIX}{}", self.request.id)
    }

    fn target(&self) -> DetailRef {
        DetailRef::Artifact {
            reference: self.request.reference.clone(),
        }
    }

    fn read(&self) -> Vec<Outgoing> {
        vec![out(
            DETAIL_GET,
            value(DetailGet {
                target: self.target(),
                etag: None,
                page: self.next.clone(),
            }),
            Some(self.tag()),
        )]
    }

    fn percent(&self) -> Option<u8> {
        percent(&self.request)
    }

    fn fail(&self, problem: String) {
        if self.percent().is_some() {
            self.request.status.try_set(Saving::Failed(problem));
        }
    }

    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn accepts(&self, tag: &str) -> bool {
        tag.strip_prefix(TAG_PREFIX) == Some(self.request.id.as_str()) && self.percent().is_some()
    }

    /// Takes one served page: checks it continues the first and answers its original bytes,
    /// for the sink, and whether more follow.
    fn accept(
        &mut self,
        request: DetailGet,
        reply: DetailReply,
    ) -> Result<(Vec<u8>, bool), String> {
        let invalid = || "corrald answered the download with an unexpected page.".to_owned();
        if request.target != self.target() || request.page != self.next {
            return Err(invalid());
        }
        let Some(DetailBody::Artifact(page)) = reply.body else {
            return Err(invalid());
        };
        // Not the encoding: a text artifact's page that is not UTF-8 arrives as bytes.
        let identity = (reply.etag.clone(), page.task.clone(), page.key.clone());
        match &self.first {
            None => self.first = Some(identity),
            Some(first) if *first == identity => {}
            Some(_) => return Err("The artifact changed while it downloaded.".into()),
        }
        let bytes = match page.encoding {
            ArtifactEncoding::Text => page.content.into_bytes(),
            ArtifactEncoding::Base64 => STANDARD
                .decode(&page.content)
                .map_err(|_| "corrald sent bytes the app cannot decode.".to_owned())?,
            ArtifactEncoding::Other => {
                return Err(
                    "corrald sent the artifact in an encoding this app cannot save.".into(),
                );
            }
        };
        self.received = self.received.saturating_add(bytes.len() as u64);
        if self.received > self.request.bytes {
            return Err("corrald sent more bytes than the artifact holds.".into());
        }
        self.hash.update(&bytes);
        match (reply.next, page.truncated) {
            (Some(next), true) => {
                self.next = Some(next);
                Ok((bytes, true))
            }
            (None, false) => Ok((bytes, false)),
            _ => Err(invalid()),
        }
    }

    /// Whether the complete read is the catalog's artifact.
    fn verify(&mut self) -> Result<(), String> {
        let digest = format!("{:x}", std::mem::take(&mut self.hash).finalize());
        if self.received != self.request.bytes || !digest.eq_ignore_ascii_case(&self.request.sha256)
        {
            return Err(
                "The downloaded bytes do not match the artifact's size and SHA-256; nothing was saved."
                    .into(),
            );
        }
        Ok(())
    }

    fn progress(&self) -> u8 {
        match self.request.bytes {
            0 => 100,
            total => ((self.received.min(total) * 100) / total) as u8,
        }
    }
}

impl AppState {
    pub(super) fn save_artifact(&mut self, request: SaveArtifact) -> Vec<Outgoing> {
        if percent(&request).is_none() {
            return vec![];
        }
        let transfer = SaveTransfer {
            request,
            next: None,
            first: None,
            received: 0,
            hash: Sha256::new(),
        };
        let problem = if !self.ready {
            Some("Downloading needs a connection to corrald.")
        } else if transfer.request.reference.contains('#') {
            Some("Only a whole artifact can be downloaded.")
        } else if self
            .save
            .as_ref()
            .is_some_and(|old| old.percent().is_some())
        {
            Some("Another download is under way.")
        } else {
            None
        };
        if let Some(problem) = problem {
            transfer.fail(problem.into());
            return vec![];
        }
        let outgoing = transfer.read();
        self.save = Some(transfer);
        outgoing
    }

    pub(super) fn cancel_save(&mut self, id: &str) {
        if self.save.as_ref().is_some_and(|save| save.request.id == id) {
            self.save = None;
        }
    }

    pub(super) fn interrupt_save(&mut self) {
        if let Some(save) = self.save.take() {
            save.fail("The connection to corrald changed before the download finished; nothing was saved.".into());
        }
    }

    pub(super) async fn save_reply(
        &mut self,
        tag: &str,
        params: Value,
        outcome: Result<Value, (wire::rpc::ErrorCode, String)>,
    ) -> Vec<Outgoing> {
        if !self
            .save
            .as_ref()
            .is_some_and(|save| tag.strip_prefix(TAG_PREFIX) == Some(save.request.id.as_str()))
        {
            return vec![];
        }
        let Some(mut save) = self.save.take() else {
            return vec![];
        };
        let Some(old_percent) = save.percent() else {
            return vec![];
        };
        let more = match outcome {
            Ok(reply) => decode::<DetailGet>(params)
                .and_then(|request| Ok((request, decode::<DetailReply>(reply)?)))
                .and_then(|(request, reply)| save.accept(request, reply)),
            Err((wire::rpc::ErrorCode::Conflict, _)) => {
                Err("The artifact changed while it downloaded. Try again.".into())
            }
            Err((_, problem)) => Err(format!("corrald could not serve it: {problem}")),
        };
        // The sink takes the page before the next is asked for: at most one page waits.
        let more = match more {
            Ok((bytes, more)) => save.request.output.append(&bytes).await.map(|()| more),
            Err(problem) => Err(problem),
        };
        match more {
            Ok(true) => {
                let percent = save.progress();
                if percent != old_percent {
                    save.request.status.try_set(Saving::Reading {
                        id: save.request.id.clone(),
                        percent,
                    });
                }
                let outgoing = save.read();
                self.save = Some(save);
                outgoing
            }
            Ok(false) => {
                if let Err(problem) = save.verify() {
                    save.fail(problem);
                    return vec![];
                }
                let status = save.request.status;
                status.try_set(Saving::Finishing);
                let result = save.request.output.finish().await;
                status.try_set(match result {
                    Ok(at) => Saving::Saved(at),
                    Err(problem) => Saving::Failed(problem),
                });
                vec![]
            }
            Err(problem) => {
                save.fail(problem);
                vec![]
            }
        }
    }
}

fn percent(request: &SaveArtifact) -> Option<u8> {
    match request.status.try_get()? {
        Saving::Reading { id, percent } if id == request.id => Some(percent),
        _ => None,
    }
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
