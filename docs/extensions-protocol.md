# Corral extension protocol v1 (draft)

Status: **draft**. Protocol v1 freezes only after the Mitigraph `task.context` provider passes
end to end and a further architect review of this document approves it (T-381 decision OD9).
Until then any part may change. Every numeric limit below is a provisional **proposal**
ceiling (OD6), not a measured budget.

This document specifies the wire contract implemented by `corral_core::extension`
(`crates/corral-core/src/extension/`). It covers the manifest, framing, envelope, methods,
errors, context attempt states, the context binding and its digest, and public-read cursors.
Registration, activation, the one-shot runner, `corral read` and dispatch admission are later
stages of T-381 and are not described here.

## Identities

| Identity | Form |
|---|---|
| Extension ID | `^[a-z][a-z0-9-]{0,31}$` |
| Package version | semver string from the manifest; display only |
| Manifest version | integer `manifest_version`, `1` |
| Wire protocol version | integer, `1`, negotiated per session |
| Capability | `<name>@<version>`, for example `task.context@1` |
| Digests | `sha256:<64 lowercase hex>` |

## Manifest

File `corral-extension.toml`, static TOML that Corral parses but never executes:

```toml
manifest_version = 1
id = "mitigraph"
version = "0.1.0"
executable = "corral-ext-mitigraph"      # relative to the manifest directory, then PATH
protocol_versions = [1]
execution_modes = ["oneshot"]           # "oneshot" | "managed"
entry = ["extension", "invoke"]         # argv appended for integrated invocation
concurrency = 1                         # max concurrent requests per managed session

[[providers]]
name = "task.context"
versions = [1]
reads = ["read.batch", "read.changes", "read.snapshot"]

[config_schema]                         # JSON Schema subset; contents are free-form

[[subscriptions]]                       # parsed, not activatable until stage 2
name = "tasks"
cursor_kind = "event_seq"               # "change_seq" | "event_seq"
filter = ["created", "moved"]

[[requested_actions]]                   # parsed, not activatable until stage 3
action = "task.create"
```

Rules:

- Required keys: `manifest_version`, `id`, `version`, `executable`, `protocol_versions`,
  `execution_modes`. `entry` defaults to empty, `concurrency` to 1.
- Unknown keys are rejected at every level except inside `config_schema`. Every rejection
  names its field by path, for example `id`, `providers[0].reads[2]` or
  `subscriptions[0].replay`.
- `version` is `MAJOR.MINOR.PATCH` with optional pre-release and build suffixes.
- Version lists are non-empty, distinct and at least 1. Provider names are unique dotted
  lowercase segments. `reads` entries come from `read.batch`, `read.changes` and
  `read.snapshot`.
- A `change_seq` subscription filters scope kinds (`task`, `lessons`, `artifact`). An
  `event_seq` subscription filters event kinds (`created`, `moved`).
- `subscriptions` and `requested_actions` parse and validate, but the manifest then reports
  them as not activatable, with stage 2 and stage 3 respectively.

## Framing

LSP/DAP style on the extension's stdin and stdout:

```text
Content-Length: <n>\r\n
\r\n
<n bytes of UTF-8 JSON>
```

- `Content-Length` is the only header. Its name is matched case-insensitively, and its value
  is decimal digits. Any other header line, a bare LF line ending, or a header line over 64
  bytes is a violation.
- The maximum frame body is 1 MiB (**proposal**). The length is checked before the body is
  read, so a reader's memory per frame is bounded by the limit. A typed response is exactly
  one frame, so this ceiling bounds it as well; v1 has no chunked responses.
- stdout carries frames only; stderr is diagnostics, captured up to 64 KiB (**proposal**).
- A one-shot exchange ends with the stream's end: any byte after the final expected frame is a
  violation. An extension that reads EOF on stdin before a complete request exits without
  output.

Every framing failure is `protocol_violation`, with a reason token in `data.reason`:

| Reason | Cause |
|---|---|
| `frame_too_large` | `Content-Length` above the limit |
| `unexpected_header` | missing or malformed `Content-Length`, an extra header, or LF-only line ends |
| `header_too_long` | header line over 64 bytes |
| `truncated_header` | stream ended inside a header |
| `truncated_body` | stream ended before `Content-Length` bytes |
| `trailing_bytes` | bytes after the final frame |
| `invalid_utf8` | body is not UTF-8 |
| `invalid_envelope` | body is not a valid envelope (see below) |

## Envelope

```json
{"corral_ext": 1, "kind": "request", "id": "A-7f3c", "method": "provider.invoke", "params": {}}
{"corral_ext": 1, "kind": "response", "id": "A-7f3c", "result": {}}
{"corral_ext": 1, "kind": "error", "id": "A-7f3c", "error": {"code": "input_mismatch", "retryable": false, "detail": "...", "data": {}}}
{"corral_ext": 1, "kind": "progress", "id": "A-7f3c", "progress": {"stage": "rank", "done": 3, "total": 9}}
```

`id` is the request (attempt) ID. A `corral_ext` other than 1 is `unsupported_protocol`. An
unknown `kind`, unknown method or malformed JSON is `protocol_violation` with reason
`invalid_envelope`. Progress is advisory, rate-limited to 4/s (**proposal**), and never counts
as completion.

## Methods and negotiation

| Method | Stage | Notes |
|---|---|---|
| `initialize` | 1 | negotiation, below |
| `provider.invoke` | 1 | `task.context@1` only in the pilot |
| `events.deliver` | 2 | invalidation and action batches |
| `request.cancel` | 4 | one-shot cancellation is by process signal |
| `command.invoke` | later | manifest-declared commands |
| `evidence.read` | later | bounded; cannot invoke providers or request actions |
| `health`, `shutdown` | 4 | managed sessions only |

One-shot: the host writes `initialize` and then exactly one work request. The extension
answers `initialize`, then the work response, then exits 0.

`initialize.params`:

```json
{"protocol_versions": [1],
 "capabilities": [{"name": "task.context", "versions": [1]}],
 "host": {"corral_version": "0.3.0", "workspace_id": "..."},
 "limits": {}}
```

`initialize.result`:

```json
{"protocol_version": 1, "capabilities": [{"name": "task.context", "version": 1}], "unsupported": []}
```

The host checks the result in this order:

1. The chosen protocol version must be both offered and declared in the manifest; otherwise
   `unsupported_protocol`.
2. Each capability may be chosen only once; otherwise `protocol_violation`
   (`duplicate_capability`).
3. A capability or version the manifest does not declare is `manifest_mismatch`, whatever the
   host offered. `data.capability` names it.
4. A declared capability the host did not offer is `unsupported_capability`.

### `task.context@1` params

```json
{"binding": {},
 "budget": {"max_items": 20, "max_bytes": 16384, "max_evidence_items": 8, "max_excerpt_bytes": 320},
 "deadline_unix_ms": 1790000000000}
```

## Errors

An error object is `{code, retryable, detail, data?}`. `retryable` defaults from the table
when absent, and the origin may override it.

| Code | Origin | Default retryable |
|---|---|---|
| `protocol_violation` | host, about extension output | no |
| `unsupported_protocol` | host or extension | no |
| `unsupported_capability` | host or extension | no |
| `manifest_mismatch` | host or extension | no |
| `registration_stale` | host | no |
| `invalid_request` | host, reader or extension | no |
| `input_mismatch` | host or extension | no |
| `superseded` | host or extension | yes |
| `cancelled` | host | no |
| `deadline_exceeded` | host | yes |
| `resource_exhausted` | host or reader | yes |
| `limit_exceeded` | host, reader or extension | no |
| `source_unavailable` | extension | yes |
| `evidence_unavailable` | host or extension | no |
| `not_available` | reader (a record kind, not an error frame) | no |
| `access_revoked` | reader or host | no |
| `cursor_expired` | reader | no |
| `lineage_changed` | reader | no |
| `conflict` | host (idempotency key reused with other content) | no |
| `recovery_required` | host | no |
| `ext_invocation_refused` | Corral CLI under the invocation marker | no |
| `internal` | any | yes, once |

A code outside this set decodes as `internal`. The original code is kept in
`data.original_code`, and the original `data`, if any, in `data.original_data`.
`config_invalid` is Corral's configuration error, not a protocol code.

## Attempt states and terminal outcomes

States run `starting -> running -> terminal`. A `starting` attempt may also end as
`cancelled`, `deadline_exceeded` or `failed` without running, for example when it is cancelled
before publication or its host is lost. Only a `running` attempt can be `accepted` or
`superseded`. Terminal states never change.

| Outcome | Encoding (`state` in attempt records, `outcome` for a bare outcome) |
|---|---|
| accepted | `{"state": "accepted"}` |
| superseded | `{"state": "superseded"}` |
| cancelled | `{"state": "cancelled"}` |
| deadline exceeded | `{"state": "deadline_exceeded"}` |
| failed | `{"state": "failed", "code": "<error code>", "reason": "<token>"?}` |

Non-terminal states are `{"state": "starting"}` and `{"state": "running"}`. Recovery after the
host or child process is lost records `{"state": "failed", "code": "internal", "reason":
"process_lost"}`. A failure built from `superseded`, `cancelled` or `deadline_exceeded`
becomes that dedicated outcome, so each terminal fact has one encoding. `reason` is a
Corral-authored token; extension-supplied text never appears in it.

## Context binding

```json
{"v": 1,
 "scope": {"workspace_id": "...", "project": "corral", "repo": "/abs/repo"},
 "task": {"id": "T-381", "spec_rev": "<sha256>"},
 "code": {"kind": "dispatch_base", "base": "main", "base_sha": "<40 hex>", "tree": "<40 hex>"},
 "workflow": {"mode": "current_at_capture", "lineage": "<uuid>", "w_min": "<cursor>"},
 "provider": {"ext": "mitigraph", "registration_rev": 3, "manifest": "sha256:...",
              "executable": "sha256:...", "capability": "task.context@1", "config": "sha256:..."},
 "policy": {"freshness": "current_at_capture", "required": false}}
```

- `code.kind` is `dispatch_base` or `candidate`. A candidate carries only `version`, `head`,
  `tree` and `spec_digest` from a CandidateIdentity, never its captured environment. The pilot
  admits only `dispatch_base`.
- `workflow.mode` and `policy.freshness` are `current_at_capture` or `exact`. An `exact`
  binding names `workflow.generation`; the field is omitted otherwise.
- Unknown keys are rejected at every level.

### Binding digest

`binding_digest = "sha256:" + lowercase_hex(SHA-256(canonical_json(binding)))`.

Only the `binding` object is digested. Deadlines, request IDs and output ceilings
(`brief_max_bytes`, the `budget` fields) are outside it, so changing one never changes the
digest. A change to any binding field, such as `task.spec_rev`, does change it.

Canonical JSON:

- Object members are sorted by key, comparing the keys' UTF-8 bytes. Array order is kept.
- There is no whitespace anywhere.
- Strings are UTF-8 and escape only `"` as `\"`, `\` as `\\`, and U+0000-U+001F. Those
  control characters use `\b`, `\f`, `\n`, `\r` and `\t` where JSON defines them, and
  lowercase `\u00xx` otherwise. Everything else, including `/` and non-ASCII characters, is
  written literally.
- Numbers are integers in plain decimal; the binding contains no floating-point values.
- Literals are `true`, `false` and `null`. Absent optional fields are omitted, never written
  as `null`.

Example: `{ "z": 1, "a": { "y": 2, "b": [3, "x"] } }` canonicalizes to
`{"a":{"b":[3,"x"],"y":2},"z":1}`.

## Public-read cursor

A cursor is opaque to consumers. Its encoding is unpadded base64url (RFC 4648 section 5) of
the canonical JSON of:

```json
{"v": 1, "workspace_id": "...", "lineage_id": "...", "project": "corral", "projection": 1, "seq": 42, "eseq": 7}
```

- `seq` is a ChangeSeq (state changes) and `eseq` an EventSeq (departure events). Both are
  captured from the same read view and are never compared with each other.
- A cursor is not a credential and carries no MAC.
- Decoding is strict and fails as `invalid_request` for:
  - text over 4096 characters;
  - padding or a character outside the base64url alphabet;
  - non-zero unused trailing bits, so each cursor has exactly one accepted encoding;
  - malformed JSON or unknown keys;
  - a missing field, including `seq` or `eseq`;
  - `v` other than 1;
  - `projection` other than 1.
- A decoded cursor is accepted only for the reader's own workspace, lineage and project.
  Another lineage is `lineage_changed`; another workspace or project is `invalid_request`.
