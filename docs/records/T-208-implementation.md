# T-208 — Claude Code native inbox adapter

## Outcome

Implemented Claude Code cross-session socket delivery for coding/planning runs, with
an immutable per-run inbox/legacy boundary. The allowlist is exactly **Linux,
Claude Code 2.1.304, peerProtocol 1**. Other versions/platforms and unrecognized
policy or transcript shapes fail closed; no paste fallback is introduced here.

## Capture and approved deviation

The capture preceded implementation: `T-208-claude-capture/` pins the accepted frame,
live registry, delivered user record and policy logs. Disposable private HOME/XDG,
a detached disposable worktree and an invalid disposable API key were used; no
owner credentials or herdr panes were used. Owned processes were waited for, the
worktree removed, and the socket disappeared. The socket delivery and transcript
write succeeded, then the model request failed with 401 (no successful model response
is claimed).

**Plan deviation authorized by PM answer T-208/R1/Q1, revision 2:** the captured
peer-user record has `isMeta: true`, contrary to the plan's blanket exclusion.
Only the version-pinned `type:user`, `isMeta:true`, `origin.kind:peer`,
`turnOrigin:peer`, bound-session, exact-wrapper shape can count. Its complete
canonical payload must hash-match the frozen envelope. Generic meta records,
assistant/tool results, summaries, sidechains, changed wrappers, wrong sessions,
unknown versions and truncated payloads do not count.

## Boundaries

- The binding's adapter-opaque session string holds native sessionId + pid + procStart;
  its endpoint and transcript path are frozen too. Registry candidates require the
  worktree, launch window, live kernel generation, pane foreground/descendant and a
  user-owned socket in a user-owned 0700 directory. Every send rechecks these facts,
  protocol and policy. Linux socket peer credentials additionally pin the receiver pid.
- Native launch arguments occupy one standalone unquoted `{delivery_args}` slot:
  `--settings '{"crossSessionInbound":"accept"}'`. Missing/quoted/duplicate slots and
  competing template/profile `--settings` arguments are rejected at profile resolution.
  Default Claude templates include the slot. Legacy run rendering removes it and does
  not add the setting; old run delivery modes are never changed.
- Policy reads use the bound process's environment and argv, not the sync process's
  HOME. User, CLI, project/local (including main-checkout local settings), managed
  file and managed drop-ins are inspected. Explicit user, repository or managed hold/refuse
  never becomes Native through CLI accept (user merge corrected in review round 1;
  see T-208-review-round-1.md). Missing accept defaults to blocked hold.
- Remote managed caches, dynamic policy helpers, unsupported policy environment
  overrides, malformed/unreadable settings and non-Linux policy inspection are
  conservatively blocked, not guessed, executed, modified or overridden. This is a
  deliberate availability limitation, not support for effective-policy attestation of
  those runtime sources.
- One bounded socket connection writes one guarded JSON line; completed write means
  Accepted, never observed. Before any bytes, a failed connection/preflight is Rejected;
  an error after partial write is Uncertain. Claude silently drops a bad session guard
  (captured); there is no on-socket rejection acknowledgement to claim. Later full
  typed transcript evidence is the only delivery receipt.
- Capabilities request the existing generic scheduler's 10-second recipient interval
  and 6-per-minute global budget. No adapter-local retry or effect logic was added.
- Peer messages confer no permission-prompt approval or authority to change settings.

## Verification before gate

- `cargo test -p corral-ops --lib delivery::adapters::claude`: 3 passed, 0.18 seconds
  execution (initial attempt interrupted during compilation; subsequent run passed).
- `cargo test -p corral-core --test profiles --test claude_delivery_profile`: 12 passed,
  0.01 seconds execution.
- `cargo test -p corral-ops --test run_ops render_command`: 3 passed, 0.15 seconds
  execution, including legacy/inbox launch rendering.
- One fake-socket journey proves guarded Accepted delivery, no receipt before observation,
  protocol/policy blocking before publication with a fallback supplied, stale generation
  and policy rejection at send, typed transcript exclusions and incremental partial lines.
  Contract units cover the exact allowlist and malformed policy/duplicate CLI options.
- Four added tests; sub-second targeted execution. Existing generic budget/uncertainty
  tests remain the authority for scheduling, retry and fallback mechanics.

Policy-source references: <https://code.claude.com/docs/en/settings.md> and
<https://code.claude.com/docs/en/managed-settings.md> (read 2026-11-04); these describe
project tightening, main-checkout local settings, managed files/drop-ins and runtime sources.

Final rebase and receipted quick gate are recorded in the run's gate artifact, not claimed
by this pre-gate report. No load test or successful authenticated model interaction was run.
