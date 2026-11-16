# Daemon-owned assistant host

`daemon_hosted:true` in the inherited launch message selects the remote owner attachment. It requires role `assistant` and `pane:null`. Ordinary launches still require the exact nonempty pane and keep their interactive behavior. The flag is not inferred from terminal availability or arbitrary environment variables.

`main.ts` creates the existing guarded provider/model runtime, commits hello, binds extensions in print mode, and leaves the authenticated Unix bridge alive. It creates no terminal, kickoff answer, idle-stop timer, or local reply. Setup/provider/extension failures remain failures. Wait for bridge hello `bound && remote_ready` before sending.

## Bridge

The existing peer-UID, generation and constant-time `ctl_cap` checks protect every command. Remote-only commands are advertised only by a remote host:

- `owner_turn`: `{entry,input,turn,text,device,via,typed_at,context?,attachments?}`. `input` is an **already journaled** canonical ID belonging to this assistant, including generation zero for cold sends. The daemon owns checking provenance and current authority before delivery. Text and optional grounded read-only context are each limited to 16 KiB. `turn` must be `get_state.turn.current + 1`, with no active turn. Context never becomes an owner instruction. `submit_input` cannot stand in for `owner_turn` by claiming `daemon-owner` origin.
- `set_model`: `{model,effort?}` selects an available model from this generation's admitted provider catalog while idle; provider changes require a new generation. Returns actual model/effort and emits `owner_model`.
- `cancel`: remote callers must supply the exact active `turn`. A stale turn cannot stop its successor. Existing interactive cancel behavior is unchanged.

An entry admits at most one model delivery. Same-entry/same-payload retries return the original disposition; changed payloads conflict. Admission metadata is flushed into the Pi session before delivery. On restart a previously admitted entry returns `delivery_uncertain_after_restart`, never an automatic second run. The daemon must reconcile its durable delivery state, not assign another entry ID after uncertainty.

`events_since` supplies actual runtime events:

- `owner_delta {entry,input,turn,offset,text}`: UTF-8 byte offset, bounded chunks.
- `owner_error {entry,input,turn,message}`: actual delivery/provider/response-bound failure.
- `owner_done {entry,input,turn,aborted,error?,model}`: actual agent settlement.
- `owner_model {model,effort}`: current runtime selection.

The aggregate answer ceiling is 256 KiB. Exceeding it emits a truthful error and aborts that exact turn, rather than leaving a provider running after the daemon stops accepting text. Bridge ring gaps retain the existing explicit reconciliation contract.

All hosts, including this daemon-owned attachment, also advertise `live_since` in
`hello.commands`. `live_since {seq}` reads a separate bounded chat ring with the same
`{generation,events,last,next,more,gap?}` pagination and authentication as `events_since`.
Its cursor is independent of `events_since` and `hello.last_seq`. It projects upstream
turn boundaries, assistant text/thinking, tool activity and queue counts without changing
the `owner_*` stream above. Exact payloads and bounds are documented in
[`corral-pi.md`](../../../../../docs/guide/corral-pi.md#live-chat-events).

## Owner attachments

An optional `attachments` array contains at most four metadata records. Each has a distinct `id` (`a1`–`a4`), `kind` (`image`, `file`, or `link`), and a `name` of at most 160 UTF-8 bytes without controls. Unknown metadata keys are refused; there are no host-path fields or inline bytes.

- Image: `media_type` is `image/png`, `image/jpeg`, `image/gif`, or `image/webp`; `bytes` is 1–2 MiB; `sha256` is 64 lowercase hex digits.
- File: `bytes` is 1–64 KiB; `sha256` has the same format; optional `media_type` is bounded metadata. Content must be valid UTF-8 without NUL.
- Link: `url` is an opaque HTTP(S) URL of at most 2048 UTF-8 bytes without whitespace, controls, or user information. Links have no byte/digest/media-type fields and are never fetched.

The host opens only `attachments/<sha256>` inside the launch generation's runtime directory. It retains directory and regular-file handles, refuses symlinks at the generation/attachment-directory/file boundary, reads at most the declared length plus one, and verifies exact length, SHA-256, and image magic or UTF-8 content. Descriptor-relative resolution uses `/proc/self/fd` on Linux and `/dev/fd` elsewhere; an unavailable descriptor filesystem refuses rather than falling back to a path-based read.

All attachment loading and model image-support checks finish **before** admission is appended, an owner input is cited, or a turn is submitted. Malformed/unavailable/tampered content throws `bad_attachment`; an image on a model without `image` in its input modalities throws `attachment_unsupported`. These are definite non-deliveries, not uncertain accepted turns. Already admitted retries retain the existing idempotency behavior and do not reload bytes.

Typed text and its canonical owner-input identity remain unchanged. After `ownerInputText` and optional read-only context, a bounded untrusted attachment block describes names with JSON quoting, digests, byte lengths, and opaque links. File content is enclosed in a digest-bearing `<attachment aN sha256:…>` fence; content containing its own fence is refused. Images are passed as SDK `ImageContent` through `HostSession.submit` to `session.prompt`'s `images` option, and retained with the host queue input. None of this data grants owner-action authority. Plain owner turns keep their original prompt and no image option.


## Owner decisions and filing

`remote-prompts.ts` opens and polls canonical `owner_prompt` permission/confirmation records and `owner_ask` questions. Reads do not retain mutation receipts. Matching-turn cancellation aborts waits; expiry, cancellation and denial are never consent. Only the authenticated daemon answer service can decide a remote prompt. Permission opens cite the initiating input ID and include the real operation, target, digest, preview and permitted scope candidates. Confirmations retrieve the prior canonical owner-action prompt bound to its exact command/request, rather than manufacturing one from text.

Remote `ask_user` waits for canonical answers (including when a model requests terminal nonblocking behavior). It preserves option labels/descriptions, consults the canonical answer after a revision change, and returns no citable owner-input ID. Question answers are conversation, not owner-action authority. No remote path calls the pane/dialog/confirmation owner-input event journal or `write_permission.decide`.

The remote-only `corral_idea_file` model tool invokes canonical `idea_file` with `{input,title,body,project?,labels?,criteria?}`. Rust owns verbatim owner-word preservation, assistant origin, live authority checks and the shared input-keyed duplicate guard. Project selection uses an explicit configured `project`, then the cited message's validated project context, then the assistant session default. Unknown context projects are ignored; unknown explicit projects are refused. The assistant receives the current project as its filing default unless the owner says otherwise. Ordinary interactive tool loadouts are unchanged.
