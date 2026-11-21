//! Credential discovery and acknowledged, restart-safe local identity removal.
use super::*;

impl Driver {
    pub(super) fn begin_pair(&mut self, code: &str, name: String, endpoint: Option<&str>) {
        if self
            .feed
            .with_untracked(|feed| matches!(feed.browser_pairing, BrowserPairing::Blocked(_)))
        {
            return;
        }
        if self.forgetting.pending() {
            #[cfg(target_arch = "wasm32")]
            let problem = "Forgetting this browser is incomplete; retry Forget before pairing.";
            #[cfg(windows)]
            let problem = "Forgetting this device is incomplete; retry Forget before pairing.";
            self.forget_failed(problem.into());
            return;
        }
        self.close_connection();
        self.app.disconnected(self.feed);
        self.feed
            .update(|feed| feed.browser_pairing = BrowserPairing::Connecting);
        let result = (|| {
            #[cfg(target_arch = "wasm32")]
            let mut code = PairCode::decode(code)?;
            #[cfg(windows)]
            let mut code = super::super::pairing_link::from_input(code)?;
            #[cfg(target_arch = "wasm32")]
            let _ = endpoint;
            #[cfg(windows)]
            {
                let supplied = socket::normalize_endpoint(
                    endpoint
                        .ok_or("Enter the daemon address shown with the pairing code.")?
                        .trim(),
                )?;
                let expected = code
                    .endpoint
                    .take()
                    .unwrap_or_else(|| format!("ws://127.0.0.1:{}", code.port));
                if supplied != socket::normalize_endpoint(&expected)? {
                    return Err("The daemon address does not match the pairing code.".into());
                }
                code.endpoint = Some(supplied);
            }
            let keys = noise::generate_keypair()?;
            let private = keys
                .private
                .as_slice()
                .try_into()
                .map_err(|_| "invalid pairing private key")?;
            let handshake = noise::initiator_pair(private, &code.daemon_public, &code.token)?;
            let endpoint = code
                .endpoint
                .clone()
                .unwrap_or_else(|| format!("ws://127.0.0.1:{}", code.port));
            let credential = WebCredential {
                v: 1,
                device: String::new(),
                name,
                private: hex(private),
                daemon: hex(&code.daemon_public),
                port: code.port,
                endpoint: code.endpoint.take(),
            };
            self.open(
                &endpoint,
                &format!("{PAIR_PATH_PREFIX}{}", code.pair_id_hex()),
                Role::Pair {
                    credential,
                    sas: None,
                },
                handshake,
            )
        })();
        if let Err(problem) = result {
            self.failed(true, Failure::Fatal(problem));
        }
    }

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
                // A credential without its marker opened on the pairing
                // form; the next launch resumes at once.
                let _ = Storage::set_item(PAIRED_KEY, "1");
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
        self.close_connection();
        self.forgetting.begin();
        self.app.disconnected(self.feed);
        let result = async {
            // Commit the intent before removing either namespace. Every later failure
            // leaves this marker in place, including a crash between steps.
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
                    #[cfg(target_arch = "wasm32")]
                    let problem = "This browser was revoked.";
                    #[cfg(windows)]
                    let problem = "This device was revoked.";
                    feed.link_lost(problem.into());
                });
            }
        }
    }
}
