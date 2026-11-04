# T-208 Claude capture — decision needed

Captured 2026-11-04 on Linux using installed Claude Code **2.1.304**, peerProtocol **1**.
Validated range is exactly 2.1.304; no other version/platform is validated.
No adapter implementation or gate has run yet.

## Method and isolation

A detached disposable Git worktree under this run's `.evidence/claude-capture/worktree`
was used with private HOME, CLAUDE_CONFIG_DIR and all four XDG directories. The probe
removed inherited Claude, Anthropic, Corral and herdr environment variables. It used
an intentionally invalid disposable API key, no owner credentials and no herdr panes.
PTY input handled only this disposable session's startup API-key and bypass-mode
notices, not peer-requested permission approvals. Each child process group was
terminated and waited for in `finally`. The disposable worktree was removed, and the
owned socket no longer exists. Private scratch homes remain in gitignored `.evidence/`.
Claude did read ancestor project guidance from the checkout; these were not patched.
The delivered user's record was persisted before Claude's API request failed with 401.
Thus receipt shape and inbound policy were captured without a successful model call.

Probe: `python3 .evidence/capture_probe.py [accept|unset|project-refuse|local-refuse]`.
The initial successful accept invocation omitted the argument. Subsequent policy
variants sent a deliberately wrong-session frame before the correctly guarded frame.
Each session was observed for 20 seconds, not a load test.

## Observed protocol

- `session.json` is a live registry capture with cwd, pid, procStart, startedAt,
  sessionId, version, peerProtocol and messagingSocketPath.
- `frame.json` is the actual accepted single JSON-line frame. `session_id` is the
  receiver-session guard, with `type: user` and `message: {role: user, content: ...}`.
- Linux debug output says authentication is optional; the single-line write worked.
- A wrong `session_id` is silently dropped with a debug warning, not an on-connection
  rejection response. The accepted socket frame itself has no custody acknowledgement.
- `delivered-user.json` pins the delivered transcript record, not tool/assistant output.
  It has `type: user`, a string content with an exact peer wrapper, **isMeta: true**,
  `origin.kind: peer`, `turnOrigin: peer`, and the bound `sessionId`. The wrapper
  expressly says peers cannot approve prompts or change permission configuration.

## Policy results

`policy.log` contains selected raw debug output from all four runs.

| Permission mode | CLI accept setting | Project/local setting | Result |
|---|---|---|---|
| bypassPermissions | yes | absent | delivered peer user record |
| bypassPermissions | absent | absent | held, cause=no-mode-asserted; no delivered user record |
| bypassPermissions | yes | project refuse | refused before attachment materialization; no delivered user record |
| bypassPermissions | yes | local refuse | refused before attachment materialization; no delivered user record |

No held message was approved. No peer message requested settings changes or tool use.
The no-record claims above describe the bounded capture, supported by explicit hold/refuse
logs, not a general proof of absence from incomplete transcript coverage.

## Conflict requiring a decision

The approved plan's Claude observation section explicitly excludes `isMeta` records.
The only validated delivered peer record is `isMeta: true`. Implementing that exclusion
literally means successfully delivered socket messages can never gain an observed receipt.
Do not silently weaken the reader. Recommend a narrow, version-pinned exception for the
captured peer-user shape: bound session, `origin.kind: peer`, `turnOrigin: peer`, exact
wrapper and complete canonical payload hash, while still excluding arbitrary meta records,
tool_result blocks, assistant records, summaries and sidechains/subagents.

Pending: approved resolution, adapter/launch integration, fake-socket and reader tests,
final rebase and the run's receipted quick gate. No test-cost increase yet.
