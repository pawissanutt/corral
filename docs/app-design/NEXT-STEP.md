# Next increment: enrolled native and browser connections

Status: implemented connection prerequisite, verified on private native-terminal and Chromium surfaces; see [verification receipt](#verification-receipt). This document retains the implementation contract and acceptance criteria. It does not authorize installing/enabling the owner's listener or answering live questions. Follows [CONNECTION-CONTRACT.md](CONNECTION-CONTRACT.md); the earlier [transport experiment](TRANSPORT-PROOF.md) was feasibility evidence, not the shipped app.

## Owner-observable result

The actual NSH app connects to a private workspace daemon from its native and web builds, using persistent enrolled identities rather than disposable bootstrap keys. Both render live health and Now. A restart preserves enrolment; reconnect restores subscriptions; revocation closes the session and prevents automatic reconnection. Neither app executes task/question mutations in this increment; trusted native device administration is explicitly supported.

This completes the production connection prerequisite only. Phase 2a is not complete until the subsequent worker/PM question journey works on desktop and web. Keep the existing UI, all destinations and phone layouts; do not redesign the app or retire classic.

## Fixed decisions

- Loopback only. Network listening is explicit and off by default; no LAN, relay, phone packaging or service installation.
- Native local enrolment uses the trusted Unix peer-UID boundary. Browser enrolment uses the architecture's single-use pairing token, Noise IKpsk2 and matching SAS confirmed on the trusted local surface. A browser cannot approve its own pending enrolment.
- Subsequent browser sessions use pinned Noise IK. No secrets in URLs, public bootstrap JSON, unwrapped localStorage or source files. The proof's key provisioning must not be copied.
- Keep the existing CLI executable/build admission checks and fallback semantics. App protocol versions and capabilities are independent of executable identity; a claimed `kind = app` never establishes authentication.
- App sessions expose only the explicit board/health read allowlist. Refuse commands, nudges, daemon administration, opaque executable-bound reads, host file access and runtime control before service dispatch. Add answer/follow-up permission only in the next increment.
- Reuse the board projector, topic paging/ACKs, health facts, frame limits and existing operation/store boundaries. No second board derivation, store writer or proxy subprocess per client.

## Work packages and ownership

New module/type names below are proposed, not existing APIs. Before editing exported symbols, inspect LSP references and update every affected caller. Follow the 800-SLOC limit with sibling tests. Freeze the small cross-package interfaces in package 1; packages 2 and 3 can then proceed independently. One integration owner owns protocol/schema edits and final verification.

### 1. Wire/session interface and identity records

**Targets:** `crates/corral-protocol/src/`, its checked-in schema, `crates/corral-core/src/records/`, `daemon_cfg.rs`, and existing store/attribution APIs as required.

- Specify typed enrolment, pending pairing, confirmation, device list/revoke and authenticated-session results, including expiry, device identity and permitted capabilities. Define errors and protocol-major/minor rules; do not turn release strings into authentication.
- Store public device identity, platform, enrolment/revocation timestamps and capabilities in a bounded record family using the existing atomic store operations. Keep daemon private identity outside the workspace in the owner state directory, with restrictive directory/file permissions and atomic creation.
- Define a server-owned session context: authenticated principal/device, transport, negotiated facilities and a read-only method policy. Do not take actor, environment or authority from app command fields.
- Freeze how a native app establishes its enrolled context on the existing Unix session before granting app-specific methods. Peer-UID verification remains mandatory; this is not protection from a hostile same-user process.
- Define byte/frame inputs and outputs for the client core and the transport adapter; preserve the existing Unix protocol for CLI/classic clients.

**Acceptance:** an enrolled and revoked identity round-trips through the canonical record path; expiry and invalid state transitions fail closed; schema is generated from Rust types. No private key enters store exports or the repository. Existing Unix clients need no change to keep operating.

### 2. Daemon enrolment and authenticated loopback endpoint

**Targets:** `crates/corral-daemon/src/{lib,server,state,topics}.rs`, new focused identity/pairing/transport modules, `crates/corral/src/daemon_cmd.rs` and CLI wiring for trusted device administration.

- Add explicitly configured numeric-loopback WebSocket listening and exact Origin allowlisting. Share authenticated RPC/topic dispatch with Unix rather than forwarding raw network input to the unrestricted Unix socket.
- Implement the complete browser pairing lifecycle: short-lived single-use token, pending claim, handshake-derived SAS, trusted-side confirmation, durable enrolment and cancellation/expiry. Burn a token on claim; never treat receipt of a claim as approval.
- Authenticate ordinary Noise IK sessions against the live device registry before accepting hello or any application payload. Enforce frame bounds, handshake/I/O deadlines, concurrency limits, rekey/lifetime and orderly shutdown independently of sync pacing.
- Refuse missing/null/unapproved Origins before upgrade, handshake payloads, text/oversized frames, invalid authentication, tampering and replay. No plaintext fallback.
- Implement local device list/revoke through the existing owner-entry/service conventions. Revocation must prevent new sessions and close existing ones without waiting for a sync pass. Never emit credentials into logs.
- Preserve the current CLI `client_mismatch` checks; do not reinterpret an app-supplied principal as trusted. Keep app command dispatch disabled explicitly, even if the daemon advertises `cmd.v1` to other clients.

**Acceptance:** pairing requires the trusted confirmation; expired/reused tokens and simultaneous claims cannot enrol a second device. Unknown/revoked keys get no application data; revocation terminates a live subscription. A paired app can obtain health/board only. CLI identity mismatch remains refused.

### 3. Shared client session and secure persistence

**Targets:** existing `apps/corral-app/src/client/`, `src/model/daemon.rs`, app manifests and new focused browser/key-store glue. Extract a pure `crates/corral-client` only as the native/browser shared session implementation, not as a speculative general framework.

- Move shared hello, protocol/capability handling, snapshot accumulation, cursor/epoch reset, pending-request deadlines and reconnect decisions out of the Unix-only implementation. Keep model/view logic free of I/O and NSH dependencies.
- Build the shared core on the root MSRV and wasm target; app glue remains on its pinned Rust 1.97 toolchain. Browser entropy is WebCrypto-backed with no insecure fallback; use the proven Snow feature selection rather than its incompatible default platform-entropy path.
- Native glue keeps bounded blocking socket work off the UI thread. Browser glue uses `web-sys::WebSocket`, preserves binary message boundaries and updates NSH signals through the supported task lifecycle. Cancel/drop must close the transport and release callbacks.
- Persist native credentials in the supported platform key store; if the documented 0600-file fallback is used, show that fact and enforce restrictive creation/containment rules. Browser persistence wraps the device private key with a non-extractable WebCrypto key in IndexedDB. Missing, corrupt or unavailable secure storage must be actionable, not silently downgraded to plain Storage.
- Keep framing generic enough for CONTROL/DATA/CREDIT without enabling unsupported streams or opaque `read.v1` documents. No new command API is exposed to the UI in this increment.

**Acceptance:** both adapters feed the same model with complete snapshots, not partial pages. Reconnect after daemon restart/epoch change replaces the old board correctly. Key identity survives an app reload/restart; a changed daemon pin or corrupt credential refuses visibly rather than auto-pairing.

### 4. Wire the actual NSH connection surfaces

**Targets:** `apps/corral-app/src/state.rs`, Settings connection/device models and screens, health/Now routing and shell entry points as required. Preserve the established components and idiom-specific layouts.

- Provide trusted native enrolment and browser pairing input/confirmation state using existing Settings/Connection surfaces. Display the pinned workspace/device and direct-loopback reach path without exposing secret material.
- Offer daemon data on web only when the authenticated browser adapter exists. Preserve Fixture and Disconnected as explicit alternatives; a failed connection must never display fixtures as live data.
- Show connecting, paired, stale, revoked, storage-failure and pin-mismatch states. Preserve the last board marked stale on loss; distinguish link state from daemon loop health. Pending pairing confers no access.
- Keep Inbox, Ask and all unserved destinations explicitly unavailable. The work is connection integration, not turning local fixture reducers into apparent daemon actions.

**Acceptance:** actual native and browser app instances show the same private fixture board and truthful connection facts. Reload/restart preserves enrolment. Revoke from the trusted surface: both the visible state and reconnection behavior reflect it. Keyboard/mouse entry points and phone layout remain usable; the sidebar stays available/collapsible at narrow widths.

## Verification, not evidence volume

Use the lowest layer for each named defect. Do not reproduce the same boundary in several expensive journeys.

- Registry/pairing: deterministic unit/service checks for token expiry/reuse, confirmation race, revocation and invalid transitions; tiny fixtures, explicit clock inputs, no sleeps or owner store.
- Protocol/admission: targeted checks for unknown protocol major, forbidden app method and preserved CLI build mismatch. Exercise bounded framing and authentication failures at the transport boundary.
- Shared client: reuse/migrate existing paging/reset coverage, adding only missing consumer-visible reconnect or persistence failures. No source-text, forwarding or wording tests.
- One end-to-end connection journey: private daemon/store/runtime environment, native app and real Chromium NSH web build, enrol → complete board → reload/restart → epoch replacement → revoke → reconnect refusal. Do not answer a live owner question or use the live Herdr server.
- Run root crate checks and app checks separately and serially. Start with changed-file diagnostics and the cheapest relevant tests. The repo's daemon/protocol gate does not build the app. Use the named Corral gate only if this plan is converted into a Run contract that requires it.
- On this host, native runtime proof uses the terminal target; GTK desktop builds are forbidden. A supported-host desktop smoke remains an explicit requirement before claiming the desktop/browser Phase 2a milestone. Browser proof must run the actual NSH app, not the disposable standalone experiment again.
- Security changes need an evidence-backed review of identity establishment, Origin handling, key storage, pairing confirmation and revocation before enabling the listener outside a private fixture. No intentional host load generation.

## Completion and subsequent work

This increment is done only when packages 1–4 work together; a listener, a paired key without an app, or another standalone crypto demo is insufficient. Update the app guide and current-state guidance to match shipped behavior and attach the actual native/browser receipts. Do not install or enable the owner's listener/service without a separate rollout instruction.

Next, implement bounded Inbox/question-context readers, canonical worker/PM questions and follow-ups, then expose only their authorized answer operations through the existing command executor. Validate one winner across devices and uncertain-result recovery. Preserve the distinction between board change sequence, owner task revision and question revision. Ask remains Phase 2b as a complete chat flow; runtime migration, relay/push and classic retirement remain later work.

## Verification receipt

Observed on 2026-11-14, using a disposable two-task workspace, a fresh plugin-free
herdr server and private daemon/state directories. No owner daemon or live herdr
session was used or configured.

- Root: protocol 7 tests, targeted daemon configuration 1 test, daemon 28 tests,
  `cargo check -p corral`, and the specified five-package all-target Clippy command
  with `-D warnings` passed.
- App: `cargo test` passed 63 tests; the actual library built for
  `wasm32-unknown-unknown`. The revoked-link regression was observed red then green:
  revocation must clear the board **and** mark the link down.
- Real `nsh run --target tui`: enrolled through Connection, loaded both fixture tasks,
  saved the device-id file with mode `0600`, and loaded the board again after restart
  without another enrolment. `--port 8791` kept its dev session separate from web 8790.
- Real Chromium app: paired through the native Devices panel with matching SAS;
  opened live data and rendered both tasks; reload restored the board without pairing.
  IndexedDB inspection showed AES-GCM, a non-extractable key and a 12-byte IV.
- Forgot the browser and paired again through `corral device pair`; the CLI displayed
  the same SAS and accepted `y`. `device list` showed the native and both web records.
- CLI revocation removed the browser's board and showed the revoked notice. Instrumented
  real WebSocket construction observed zero reconnect attempts for more than two minutes.
  Native revocation and revoked attach both showed the notice; the native health/link
  state was also verified down after the regression fix.
- The integrated daemon test covered reused pairing rejection, both board rows,
  forbidden command/nudge/read/stop requests, live revoke, unknown-key reconnect
  refusal and identity-file permissions. A real unexpected-Origin upgrade returned 403.
- Browser surfaces were inspected at 1280×900 and 390×844, including the revoked
  Connection recovery control. The Settings mechanical detector reported no findings.

The private foreground daemon truthfully reported degraded (`unsupervised`, PM absent);
this was not presented as healthy. GTK/native-window backends remain unverified on this
host, and the worker/PM question milestone remains unimplemented.
