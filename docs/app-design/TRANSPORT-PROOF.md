# Browser transport feasibility: observed results

Status: completed disposable experiment for the [connection contract](CONNECTION-CONTRACT.md), not production app wiring. Baseline: branch `app-daemon-integration`, Corral commit `6f91ec1175e1ac1edc5ff5a568dd6620e4de824b`.

## What actually ran

A native Rust responder accepted WebSocket connections on `127.0.0.1:18798`, restricted to Origin `http://127.0.0.1:18797`. It authenticated ephemeral client keys with `Noise_IK_25519_ChaChaPoly_BLAKE2s`, then relayed an explicit read-only method/topic allowlist to the installed daemon's Unix socket. It used the worktree's actual `corral-protocol` crate for framing and typed request validation. It rejected all other RPCs before the sole Unix write path.

A real Chromium page on the allowed loopback origin instantiated a Rust `wasm32-unknown-unknown` module containing `snow 0.10.0`. Browser JavaScript supplied WebSocket I/O and `crypto.getRandomValues`; it did not implement or simulate Noise. The wasm module performed the initiator handshake and transport encryption/decryption. The native responder used the same `snow` release. Both handshake payloads were empty; application hello followed authentication.

The browser subscribed to health and board, accumulated all pages, acknowledged every page and consumed the ACK responses before declaring completion. It then closed the connection. The responder logged six forwarded requests and nine daemon frames for the successful snapshot, consistent with hello, two subscriptions and three page ACKs.

## Observed checks

| Check | Result |
|---|---|
| Rust wasm build | Passed with rustc/cargo 1.97.1, `--target wasm32-unknown-unknown --release` |
| Native responder build | Passed with Rust 1.97.1; no root workspace or NSH build was run |
| Browser secure context | `isSecureContext = true`; WebCrypto randomness available |
| Rust/wasm → native Noise IK | Authenticated |
| Real daemon board | 174 tasks, two complete acknowledged board pages |
| Real daemon health | `healthy` |
| Capability requested/negotiated by this proof | `board.v1`; this is not the daemon's complete capability inventory |
| Browser page errors on successful snapshot | None reported |
| Unknown client static key | Closed 1008, `noise_auth_rejected`; zero Unix requests forwarded |
| Wrong pinned server key | Closed 1008, `noise_auth_rejected`; zero Unix requests forwarded |
| Tampered transport ciphertext | Closed 1008, `noise_transport_rejected`; only the preceding valid hello was forwarded |
| Replayed transport ciphertext | Closed 1008, `noise_transport_rejected`; only the preceding valid hello was forwarded |
| Forbidden RPC (`proof.forbidden`) | Closed 1008, `rpc_not_allowlisted`; only the preceding valid hello was forwarded |
| Missing Origin | HTTP 403 before WebSocket upgrade |
| Origin `null` | HTTP 403 before WebSocket upgrade |
| Origin `http://evil.example` | HTTP 403 before WebSocket upgrade |

Negative browser cases counted only explicit authentication failures or peer policy closes, not timeouts. The forbidden RPC was deliberately nonexistent and harmless even if accidentally forwarded. No live question was answered, no task was edited, no sync was nudged and no mutation was sent to the daemon. Task count is the snapshot observed during this experiment, not a fixed acceptance value.

## Build finding: browser entropy without a bindgen dependency

The successful wasm dependency configuration was:

```toml
snow = { version = "=0.10.0", default-features = false, features = [
  "default-resolver", "use-curve25519", "use-chacha20poly1305", "use-blake2"
] }
```

A custom `snow::resolvers::CryptoResolver` supplied `snow::types::Random::try_fill_bytes` through an imported browser `crypto.getRandomValues` function. DH, hash and cipher resolution delegated to `DefaultResolver`. Entropy failure returned `snow::Error::Rng`; there was no deterministic or insecure fallback.

This avoids enabling Snow's default `std`/`use-getrandom` path, which pulls in the platform entropy dependency. The standalone proof used a raw wasm ABI and did not need the absent `wasm-bindgen` CLI. The release wasm artifact was 105,913 bytes; that is this experiment's module, not the size of a shipped NSH app.

For production, browser glue may use `web-sys`/wasm bindings rather than this disposable raw ABI. The finding is that the selected Noise implementation and cryptographic randomness work in a real browser, not that an FFI design must be copied unchanged.

## Safety and limits

- The proof used throwaway keys with manually provisioned ephemeral bootstrap data. Its temporary directory was owner-only. The HTTP-served bootstrap included a private test key to make the experiment self-contained: **this is not an acceptable production credential-distribution or pairing mechanism**.
- No daemon, app or protocol production source was changed. Nothing was installed, enabled or committed. Both temporary listeners and the browser tab were stopped, and temporary sources, keys and build artifacts were removed after evidence capture.
- The existing app remains CONTROL-only on its native Unix client; its web build still has no daemon transport. This was not run inside NSH's UI/runtime, and it does not prove NSH signal/lifecycle integration or a combined app wasm build.
- Persistent device identity, secure browser key wrapping, local enrolment, Noise IKpsk2 pairing, revocation of live sessions, independently versioned app admission, DATA/CREDIT handling, rekey/liveness policy and UI delivery remain production work. This experiment did not exercise those facilities, phone platforms, high-risk confirmations or app mutations.
- The HTTP Origin rejection cases were raw upgrade probes; the successful Noise and ciphertext rejection cases ran in the real browser. No load generation or broad repository gate was run.

## Implementation consequence

The architecture's `snow`-on-wasm feasibility question is resolved for the selected primitive suite. Proceed with production device/session identity and a shared frame/session core, keeping Unix CLI executable/build checks intact. Then connect the same protocol to native NSH and browser `web-sys::WebSocket` glue and verify the real app on both carriers. Do not expose a network listener by copying the experiment's bootstrap key distribution. Inbox/question readers and answer/follow-up commands follow that authenticated seam; Phase 2a still requires the desktop and browser journeys together.
