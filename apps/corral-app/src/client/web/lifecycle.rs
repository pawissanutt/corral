//! Credential discovery and acknowledged, restart-safe local identity removal.
use super::*;

impl Driver {
    pub(super) async fn restore(&mut self) {
        // NSH's paired marker is not authoritative and may disappear independently.
        let explicit_pairing = self.feed.with_untracked(Feed::browser_pairing_pending);
        match keystore::load()
            .await
            .map(|stored| stored.for_startup(explicit_pairing))
        {
            Ok(keystore::Stored::ForgetPending) => {
                let pairing_code = self.feed.with_untracked(|feed| feed.pairing_code.clone());
                self.forgetting.begin();
                self.forget().await;
                if !self.forgetting.pending() {
                    self.feed.update(|feed| feed.pairing_code = pairing_code);
                }
            }
            Ok(keystore::Stored::Credential(credential)) => {
                let device = credential.device.clone();
                self.credential = Some(credential);
                match self.app.boot(self.feed, Some(&device)).await {
                    Ok(()) => self.connect_session(),
                    Err(problem) => self.failed(false, Failure::Fatal(problem)),
                }
            }
            Ok(keystore::Stored::Missing) => {
                Storage::remove_item(PAIRED_KEY);
            }
            Err(problem) => self.failed(false, Failure::Fatal(problem)),
        }
    }

    pub(super) fn forget_failed(&mut self, problem: String) {
        self.connection = None;
        self.retry_at = None;
        self.app.disconnected(self.feed);
        self.feed.update(|feed| {
            feed.device = DeviceLink::Failed(problem.clone());
            feed.browser_pairing = BrowserPairing::Failed(problem.clone());
            feed.link_lost(problem);
        });
    }

    pub(super) async fn forget(&mut self) {
        self.connection = None;
        self.retry_at = None;
        self.push_intent = None;
        self.forgetting.begin();
        self.app.disconnected(self.feed);
        let result = async {
            // Commit the intent before removing either namespace. Every later failure
            // leaves this marker in place, including a browser crash between steps.
            keystore::begin_forget().await?;
            self.app.forget_local(self.feed).await?;
            keystore::clear().await?;
            keystore::finish_forget().await
        }
        .await;
        if let Err(problem) = self.forgetting.complete(result) {
            self.forget_failed(problem);
            return;
        }
        let blocked = self.feed.with_untracked(|feed| match feed.browser_pairing {
            BrowserPairing::Blocked(line) => Some(line),
            _ => None,
        });
        self.credential = None;
        self.app = AppState::new();
        Storage::remove_item(PAIRED_KEY);
        self.feed.update(|feed| {
            *feed = Feed::default();
            if let Some(line) = blocked {
                feed.browser_pairing = BrowserPairing::Blocked(line);
            }
        });
        self.tick_clock();
    }

    pub(super) async fn revoke_local(&mut self) {
        let device = self
            .credential
            .as_ref()
            .map(|credential| credential.device.clone());
        // Revocation must not leave drafts, snapshots or pending sends available to
        // a later identity. Use the same durable tombstone as explicit Forget.
        self.forget().await;
        if !self.forgetting.pending() {
            if let Some(device) = device {
                self.feed.update(|feed| {
                    feed.revoked(device);
                    feed.link_lost("This browser was revoked.".into());
                });
            }
        }
    }
}
