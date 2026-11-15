# App connection contract

Status: enrolled native Unix and paired Noise WebSocket sessions implement the app connection and typed domain APIs. Private terminal and packaged Chromium smoke has covered pairing and live board/Settings, not every journey or the full gate; see the [app guide](../../apps/corral-app/README.md). Product authority remains [REQUIREMENTS.md](REQUIREMENTS.md), especially D7, D11, D17–D18 and R3, R8–R10, R17. This contract grants no permission to act on live owner data.

## Boundaries

- One owner, one workspace daemon, independently versioned clients. Native and browser clients consume the same typed protocol and service operations; neither opens the store.
- Keep the Unix CLI path, its peer-UID checks, executable/build identity checks and provably-not-accepted fallback unchanged. A client declaring `kind = app` is not a credential and must not bypass those checks.
- Reuse `corral-protocol`, the command router, its journal and task/question preconditions. Do not add a second command executor or make the browser shell out to the CLI.
- App access is authenticated and authorized per enrolled device, with an app-safe action allowlist and server-derived provenance. The temporary proof is not a deployable gateway.

## Connection and identity

| Property | Native local app | Browser |
|---|---|---|
| Carrier | Existing Unix socket | Explicit numeric-interface WebSocket; remote browser access uses operator-provided TLS/WSS |
| Bootstrap | Explicit owner-selected socket; verify peer UID | Secure origin (`http://127.0.0.1` or HTTPS); exact configured Origin allowlist |
| Identity | Enrol a device over the trusted Unix channel after peer verification | Owner-authorized enrolment binds a device public key and the daemon's pinned static key |
| Application session | Server-issued device context bound to the enrolled local connection; no identity accepted from command fields | Noise IK authenticates the device static key against the registry before any application request |
| Storage | Private 0600 device-id file, visibly identified (not an OS keychain) | Encrypted IndexedDB credential with non-extractable WebCrypto wrapping key; no long-lived secret in URLs or NSH Storage. Short-lived pairing codes may use a fragment that the app clears before pairing. |
| Reconnect | Re-establish identity and subscriptions | Fresh Noise IK, then hello and subscriptions; no resumption from an unauthenticated cache |

The local UID trust boundary remains explicit: this is not protection against a hostile process already running as the owner. Device enrolment and audit still distinguish legitimate app surfaces. Browser Origin checking is an additional boundary, never authentication by itself. Reject missing, `null`, unexpected or insecure Origins before upgrading; do not reflect arbitrary origins or enable wildcard CORS. Never expose an unencrypted WebSocket-to-Unix proxy.

Pairing uses single-use Noise IKpsk2 and trusted-side SAS confirmation; ordinary sessions use pinned Noise IK. Revocation closes sessions and prevents automatic reconnect.

Direct reach remains opt-in: absent `[daemon] listen` opens no TCP socket. One concrete
unicast IPv4/IPv6 address may be configured; wildcard and multicast binds are rejected.
Remote browser pairing additionally requires `web_endpoint = "wss://host[:port]"` and an
HTTPS-served app on an exact `web_origins` entry. A TLS reverse proxy or Tailscale Serve
preserves Origin and forwards the protocol paths; Noise remains end-to-end inside TLS.
Plain LAN HTTP origins remain invalid, with no insecure-origin override.

`PairOffer.reach` carries `ReachHint { reach, endpoint }`; its existing fragment-only URL
is suitable for QR encoding. Version-2 pairing codes carry the endpoint alongside the
pinned daemon key, token, port and pair id; version-1 loopback codes still decode.
`DeviceInfo.last_seen` and `.reach` are additive/serde-default facts. Last seen is coarse
authenticated activity (not availability); device listings report the last final hop,
while `app.session.device.reach` is the current SessionGuard's carrier. A local TLS
terminator is observed as loopback; no untrusted forwarded header asserts remote reach.

`push.v1` is an additive authenticated-device capability: `push.status`,
`push.subscribe` and `push.unsubscribe` apply only to the session's own device.
Subscriptions and selected `PushKind`s persist per device, opt-in/off by default, and
revocation clears them. `PushStatus` exposes availability, the public VAPID key,
enabled kinds, last push-service acceptance and a sanitized failure, never endpoint/key
secrets. The daemon's fixed Web Push payload is ECE-encrypted; the shipped browser
module service worker displays content-free text and opens/focuses the app. Browser
notification permission starts synchronously from the opt-in gesture. The push-service
origin is explicitly allow-listed, redirects are refused, I/O is deadline-bounded,
and the durable per-device reservation limits sends to one per minute. It watches
canonical open owner moves, not transport presence or fixture data.

Web Push needs same-origin HTTPS/loopback hosting of the module script, browser support
and the browser vendor's external push service. It does not create remote reach or a
dead-man. Relay rendezvous, native publisher APNs/FCM delivery and detecting an absent
daemon still require independently hosted TLS infrastructure; see the
[operator setup and limits](../guide/daemon.md#opt-in-web-push).

## Version and feature negotiation

- Preserve protocol-major negotiation in `hello`/`welcome`. Unknown major: refuse before opening subscriptions. Minor changes are additive; optional capabilities are intersected, not assumed from an app release string.
- App version is diagnostic metadata. Compatibility is protocol/schema/capability based, not equality with the daemon executable, a reported `ClientBuild`, or the latest app release.
- Preserve current `board.v1` row/cursor semantics (`BoardCursor { rev, after }`), including paginated replacement snapshots and epoch resets. Keep acknowledging each page before requesting more.
- Negotiate every newly implemented UI facility. Unsupported Inbox/detail/answer facilities remain visibly unavailable; never fill gaps with fixture data. A daemon advertising `cmd.v1` alone does not authorize app mutations or imply that every CLI verb is app-safe.
- Existing `read.v1` documents are executable-bound PM/classic output. Do not weaken their identity check to serve the app. Add bounded typed UI readers for question context instead of exposing executable-bearing classic documents to independently versioned clients.

## Wire and browser execution

Use `Noise_IK_25519_ChaChaPoly_BLAKE2s` (`snow`) with no handshake payload. The responder checks the remote static key and its non-revoked device record before accepting any application message. The initiator pins the daemon static key. No command, including `hello`, is eligible in a replayable IK handshake payload.

One WebSocket binary message is one Noise transport message. Its plaintext is exactly `kind:u8 | stream:u32le | payload`; do **not** include the Unix socket's outer length prefix. Preserve the shared limits: CONTROL payload at most 60 KiB, DATA at most 32 KiB, CREDIT exactly four bytes. Enforce ciphertext, plaintext and message-kind bounds before allocating/dispatching application bodies; reject text frames and incomplete messages. Cap upgrade, handshake and frame deadlines and total concurrent connections. A failed MAC, replay, malformed frame or protocol violation closes that connection, with no plaintext fallback.

The client core owns protocol state and accepts complete frame bytes. Native glue supplies Unix I/O; browser glue supplies `web-sys::WebSocket` and cryptographic randomness from the browser. NSH signals are updated by the glue, never from blocking network work on the UI thread. Typed app requests and bounded paginated results use CONTROL envelopes; per-topic backpressure and bounded caches prevent unlimited buffering.

Production sessions follow the architecture's rekey/lifetime policy (hourly or after 2^20 messages), bounded liveness checks and fresh handshakes on reconnect. Revocation closes active sessions and is rechecked before executing a queued mutation. A healthy socket is not proof that the daemon loop or projection is fresh; show daemon degradation and last successful synchronization separately.

## Command admission and provenance

Authorization comes from a server-owned connection context, not `Caller`, `ClientKind`, an environment snapshot or a browser-supplied device id:

1. The existing CLI route remains executable/build-bound and establishes its local principal from the Unix peer.
2. The app route establishes its device principal from enrolment/session authentication. It derives `actor = owner`, device-bound `via` and event provenance on the server. Browser requests cannot impersonate the PM, an assistant, a different device or a local CLI client.
3. Both routes call the existing typed command executor with the same preconditions and authority resolver. App admission rejects unsupported verbs before scheduling them. Do not forward browser-provided `HOME`, `PATH`, cwd, Herdr socket, executable or file paths as host authority.
4. App admission uses an explicit allowlist and the canonical services, preconditions and confirmation rules. It does not grant arbitrary host administration, shell or file access.
5. Per-device assistant instruction capability and direct question-answer permission are distinct. Newly enrolled native devices and paired browsers have all authority flags, including `admin` and `assistant_act`, editable per device. Confirm-class actions use a fresh device-bound preview and confirmation.

Keep `cmd_id`, expiry, digest checking, reserved/settled state and `cmd.result` recovery. After a possibly accepted submit, never resubmit with a new id or execute locally. A disconnected app has no local-store fallback. Report an unknown result distinctly from a definite refusal.

Question answers race on the canonical question revision; follow-ups also change that revision. Do not substitute `BoardRow.rev` (a projection change sequence) for the question revision or for `Expect::TaskRevision` (the owner's task revision). A stale answer is a conflict, not an automatic retry. Bounded assistant messages and drafts can persist offline with their original IDs; other offline actions are refused. Queue policy cannot create authority absent at delivery.

## Acceptance boundaries

The following remain acceptance criteria rather than a claim that all surfaces have been exercised:

- Native and browser negotiate protocol/capabilities independently of the daemon's build identity; old CLI mismatch checks still reject mismatches.
- Enrolled identities survive restart through the appropriate secure key store, and an unknown or revoked key cannot obtain application data. Revocation terminates an existing session.
- Unexpected Origin, wrong pinned daemon key, unauthenticated client, handshake payload, text/oversized frame, ciphertext tampering and replay fail closed. No command bytes reach a service before authentication and authorization.
- Both carriers deliver complete bounded board/health snapshots and recover from epoch reset and reconnect without fixture leakage.
- The question journey includes bounded context, evidence, follow-up and history for worker and PM questions. A stale answer loses cleanly; reconnecting after uncertain acceptance cannot cause a second effect.
- Exercise the real desktop and browser surfaces on private question fixtures. Keep the classic UI until actionable parity; phone layout support remains a product constraint even though phone packaging is later.

## Feasibility evidence

The transport experiment and its measured outcomes are recorded separately in `TRANSPORT-PROOF.md`. That record must distinguish Rust/wasm crypto and browser WebSocket feasibility from NSH integration, secure persistent keys, pairing and production daemon admission. No production readiness claim follows from the experiment alone.
