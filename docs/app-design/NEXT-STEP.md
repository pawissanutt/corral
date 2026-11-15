# App: current state and remaining work

Status: 2026-11-15. This replaces the completed connection-only increment.
It is not a Run contract or permission to install services or act on owner data.
Setup and current limits: [app guide](../../apps/corral-app/README.md).
Security/wire boundary: [connection contract](CONNECTION-CONTRACT.md).

## Implemented

- Enrolled native Unix and paired Noise WebSocket clients share typed reads and
  authorized actions across Inbox, Now, Changes, Plans, Agents, Review, Timeline,
  Usage, Settings and Ask. The app-local client depends on `corral-protocol`;
  no separate `corral-client` extraction is required.
- Browser and native have the same per-device authority. Any admin device pairs,
  revokes and edits authority; self-raise is refused. Admin grants and Confirm/Admin
  commands require a fresh matching preview and device-bound proof. The check is
  the owner's tap plus daemon validation, not a biometric/PIN assertion.
  Authority-change notifications invalidate pending confirmations.
- Worker/PM questions include context, evidence, follow-ups and canonical revision
  checks. Shared action tables expose record commands through menus, inspector,
  phone surfaces and palette. Review is unified below about 1100 pt content width.
- Ask is one assistant/PM chat composer. Daemon-hosted assistants stream replies;
  messages keep their IDs through recovery. Optional `assistant.summary_profile`
  selects the quick-thread summary model at the next host start; paid-API summary
  profiles are refused for lack of independent spending authority.
- Acknowledged local transactions precede send acceptance/draft retirement and
  command submission. Per-device/store outboxes support visible cancellable queued
  messages; credential-checked offline boot restores complete last-known snapshots
  as stale. Revision/CAS handling protects concurrent instances. Other actions
  refuse offline.
- Disconnected destinations offer Connect/Preview, Settings retains This device
  preferences, and sample preview is explicit and never restored on restart.
  Missing facts and disconnected state never become sample facts.
- Direct LAN/Tailscale binding is opt-in. Remote browsers require an HTTPS app,
  exact Origin allowlist and `daemon.web_endpoint` behind operator TLS/Tailscale
  Serve; no embedded TLS is supplied. Browser Web Push is opt-in, content-free
  and depends on configured VAPID credentials/vendor push services.
- `stream.terminal.v1` supplies acknowledged, bounded pane-terminal replacement
  windows with scrollback and coalescing. Session Send/Cancel are Act-class; raw
  Keys are Admin plus fresh proof. Daemon-hosted assistants have no raw terminal.
- Native desktop global Ask shortcut, close-to-hide and tray recovery/Quit are
  implemented. `corral ask` submits assistant/PM messages from the local CLI.
- Usage/Timeline/burn-up return bounded partial results with completeness, not
  fabricated totals. Quota facts remain explicitly unknown without a source.

## Remaining next steps

### Hosted infrastructure: relay and native push — blocked

Relay rendezvous/multiplexing, ticket/identity registration, independently hosted
TLS/dead-man service and native APNs/FCM publisher credentials/tokens are absent.
Direct reach and browser Web Push do not implement these components. A stopped
daemon cannot announce its own absence. Infrastructure ownership/deployment and
native publisher provisioning must precede an end-to-end relay/native-push claim;
see [architecture §9](ARCHITECTURE.md#9-relay-and-push).

### Inbound OS share targets — blocked by NSH 0.5.0

The packaged host exposes no inbound text/link/file share payload API:

- Android routes notification/URL intents and rejects non-`ACTION_VIEW` payloads:
  [`AppActivity.java`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-platform-android/java/com/longbridge/nsh/AppActivity.java#L21-L53),
  [`NshView.java`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-platform-android/java/com/longbridge/nsh/NshView.java#L610-L613).
- iOS receives URL contexts, not share-extension payloads:
  [`app.rs`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-platform-ios/src/app.rs#L148-L164).
- Web packaging emits a fixed manifest without `share_target`, with no supported
  manifest override:
  [`web.rs`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-cli/src/web.rs#L1260-L1287),
  [`WebConfig`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-cli/src/package.rs#L532-L556).

This needs upstream host/packaging support before receiver integration. The preview
share sheet is only an illustration; paste, file picking and Ask attachments work
without pretending to register an OS target.

### Supported-host and real-device verification — outstanding

The earlier private terminal/packaged-Chromium smoke established pairing, identity
restart, board/Settings reads, revocation and reconnect refusal, not the later
audit changes or a full release gate. No current claim of desktop/physical-phone,
remote TLS or vendor push delivery follows from it.

On private fixtures, the integration owner must run the root and app checks
separately and serially, then exercise:

- Actual `nsh run --target tui` at 80×24 and 120×36; navigation/focus, stacked
  lists, sidebar toggle, Ask, confirmation and live terminal lifecycle.
- Real desktop hardware/backend: shortcut registration/refusal, hidden-window
  recovery, tray Quit and single-instance behavior. This host lacks GTK 4;
  do not attempt its forbidden desktop/Linux/GPUI builds.
- Physical phone/browser and native packaging on supported hosts: pairing QR,
  portrait/landscape, keyboard/safe areas, question/plan/review actions, authority
  grants/revocation and reconnect. Phone layout is already in scope.
- Remote HTTPS/WSS through the intended proxy/Tailscale path, exact Origins,
  credential persistence, revocation and configured browser push delivery.
- Offline restart/stale snapshot, failed persistence retaining words, queued
  cancellation/delivery races and two-tab updates against private data.

Do not repeat the transport feasibility experiment or re-plan completed readers,
chat and command routing. Keep classic `corral-tui` until actionable parity and
supported-device verification justify retirement. Runtime migration and
multi-machine execution remain separate architecture work, not prerequisites for
documenting the app's current capabilities.
