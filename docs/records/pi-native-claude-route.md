# Pi-native Claude route probe (T-306, OD5)

**Decision: GO** for the question asked: upstream Pi 1.3.0 hosted by Bun reaches Claude through an
Anthropic Pro/Max OAuth login, and Anthropic's own response headers attribute the usage to the
subscription's five-hour window, not to extra usage. This record changes no default and no
allowlist. The pi-native vendor allowlist (`[openai]` in `crates/corral-core/src/profile.rs`) stays
unchanged until the follow-up named below lands.

## Authorization and budget

- Owner budget recorded on T-306 (comment 2026-10-24T03:11:01.406036577Z, answered in the run's pane):
  at most 20 requests and 50K output tokens, $0 API (subscription only).
- Used: 2 requests and 66 output tokens (9 + 57), both `max_tokens` 128, model `claude-haiku-5-5`. The
  persistent ledger (`claude-live-budget-958e6f424aff.json`) shows both settled; no API key header was
  ever sent (the guard refuses one) and API spend is $0.

## Method

- Pi stays pinned at `@earendil-works/pi-coding-agent` 1.3.0 (commit `71023aec`) on Bun 1.4.2 with
  the T-298 lock; node, npm and npx are shimmed to exit 127 and the environment is cleared (`env -i`).
- The owner logged in once through Pi's own Anthropic OAuth flow (`login.mjs`, copy-code). Tokens live
  in `~/.local/share/corral/pi-native-probe/claude/auth.json` (0600, directory 0700), written and
  refreshed only by Pi. No OmO, Claude Code, `~/.claude`, `~/.codex` or other harness credential was read,
  copied or written; the store refuses directories named `.claude`, `.codex`, `.omo`, `.pi` or `.opencode`.
- `probe.mjs` resolved the route through `ModelRuntime` (provider `anthropic`, `isSubscription` true,
  auth source `OAuth`) and sent two streamed requests through `guard.mjs`, the only path to
  `https://api.anthropic.com/v1/messages`: a plain reply and a tool call (`probe_echo`). The guard
  validates the OAuth bearer, the body and the 256-token cap and reserves the request and token budget in a
  ledger before dispatch.
- Billing is read from Anthropic's unified rate-limit headers (`classify.mjs`): a representative claim
  naming a subscription window is subscription evidence; a claim of `overage`, an overage status of
  `in_use`, or an affirmative `anthropic-ratelimit-unified-overage-in-use: true` is extra usage and vetoes
  everything else, even when the quota headers are missing; anything else is no evidence. GO also requires a completed tool-call round trip.
- Offline proof: `bash tools/pi-probe/claude/run.sh offline` runs 12 tests (guard refusals, ledger
  limits, classifier, credential store, and the full pinned-Pi path against a fake Anthropic transport) and
  an offline probe that always reports `NOT RUN`.

## Evidence

Both live requests returned HTTP 200 with identical unified headers:

| Header | Value |
|---|---|
| `anthropic-ratelimit-unified-status` | `allowed` |
| `anthropic-ratelimit-unified-representative-claim` | `five_hour` |
| `anthropic-ratelimit-unified-5h-status` / `-utilization` | `allowed` / `0.06` |
| `anthropic-ratelimit-unified-7d-status` / `-utilization` | `allowed` / `0.6` |
| `anthropic-ratelimit-unified-overage-status` | `rejected` |
| `anthropic-ratelimit-unified-overage-disabled-reason` | `org_level_disabled` |

Request 1 stopped normally (`stop`); request 2 completed with `toolUse` and a parsed tool call. The
request ids were `req_011CgLMJtTqmWHD1J2Bz1JNQ` and `req_011CgLMJyVDioqFtcQGgc6ab`.

Reading: requests succeeded and were counted against the five-hour subscription window. Extra usage
could not have paid for them on this account, because it is disabled at the organization level. That
excludes the extra-usage outcome here; it does not show how an account with extra usage enabled is billed.

## Review round 1: overage-in-use signal

The PM review found that the first classifier only looked at `overage-status: in_use` and missed the
separate `anthropic-ratelimit-unified-overage-in-use: true` header, which upstream Pi reports
(earendil-works/pi issue 10019) on subscription-auth requests while extra usage is active. A completed
response carrying that header with a `five_hour` claim classified as GO. It is fixed: the paid-usage
signal is checked before the missing-claim/status early return and before subscription classification,
classifies as extra usage (NO-GO, reason `extra_usage_billing`, which also outranks an unrelated failed
request when the reason is chosen), and is kept as `overageInUse` in the compact decision evidence.
Regressions cover the classifier (header true in any case, header alone without quota headers, header
`false` staying subscription, the evidence field) and the fake-transport probe path (one request with the
header, and both requests with only that header).

Re-classifying the two captured live observations offline (no network, no new live call) from the saved
`claude-live-result.json` gives the same result: both requests `subscription`, `overageInUse: null`,
`overageStatus: rejected`, decision **GO**, reason `subscription_coverage_evidenced`. The guard keeps every
`anthropic-ratelimit-*` response header, so a sent `overage-in-use` header would have been recorded; neither
live response carried one. The decision above is therefore unchanged. The capture is attached as
`claude-live-reclassify.json`.

## Limits of this evidence

- Two requests on one account and one small model (`claude-haiku-5-5`). Larger models, long contexts,
  sustained load, the five-hour window running out, and token refresh by Pi were not exercised.
- Billing is inferred from Anthropic's headers, not from a billing console or invoice. Utilization read
  0.06 on both requests, so no per-request increment was observed at the header's precision.
- Upstream Pi sends Claude Code identity for OAuth tokens (a Claude Code system prompt and
  `claude-cli`/`x-app: cli` headers). Whether Anthropic's terms or enforcement treat subscription use through
  a third-party harness differently is a policy question this probe cannot answer; the follow-up asks the
  owner to accept that risk before the allowlist changes.
- The probe drives `ModelRuntime` under Bun, as the T-298 live reuse check did, not `InteractiveMode` in a
  herdr pane. Pane control is covered by T-298's native proof, which this probe does not repeat.

## Outcome and follow-up

GO files a reviewed follow-up that changes the pi-native vendor allowlist; it is filed as an Idea so the
owner decides before anything runs (T-357). Until it lands, anthropic-vendor
pi-native profiles stay refused and Claude work stays on OmO. The probe login remains in the probe-owned
directory above; it is separate from the production corral-owned Pi auth directory that OD8 assigns to
`corral agent login`, and the owner can delete it at any time.

Raw output is attached to T-306: the live and offline result files, the budget ledger and the offline lane
log. Reproduce the offline side with `bash tools/pi-probe/claude/run.sh offline`; a rerun of the live side
needs a new owner authorization and budget.
