# claude-route-probe (T-306, OD5)

Question: does upstream Pi (pinned 1.3.0, hosted by Bun) reach Claude through the owner's Anthropic
Pro/Max OAuth login, and does that usage bill the **subscription** or **extra usage**? GO only with
positive evidence of subscription coverage. NO-GO keeps the refusal of anthropic-vendor pi-native
profiles. The result is recorded in `docs/records/pi-native-claude-route.md`.

Budget (recorded on T-306 by the owner): at most 20 requests and 50K output tokens, $0 API. The probe
plans two requests (a plain reply and a tool call, `max_tokens` 128) and the guard hard-caps each
request at 256 tokens.

## Pieces

- `guard.mjs`: the only path to Anthropic. Validates endpoint, OAuth bearer (an `x-api-key` header or a
  non-`sk-ant-oat` token is refused), streamed body and token cap, then reserves request count and
  output tokens in a persistent per-authorization ledger *before* dispatch. Keeps only
  `anthropic-ratelimit-*`, `request-id` and `retry-after` response headers as evidence.
- `classify.mjs`: reads billing from the unified rate-limit claim. A subscription window claim
  (`five_hour`, `seven_day*`) is subscription evidence; an `overage` claim, overage status `in_use` or
  `anthropic-ratelimit-unified-overage-in-use: true` is extra usage and vetoes the rest; anything else, or a
  billing/permission error, is NO-GO. GO also needs a completed tool-call round trip.
- `store.mjs`: Pi's `CredentialStore` over a 0600 `auth.json` in a 0700 probe-owned directory
  (`$XDG_DATA_HOME/corral/pi-native-probe/claude`). Directories named `.claude`, `.codex`, `.omo`, `.pi`
  or `.opencode` are refused, so no other harness's credentials are read or written.
- `login.mjs`: the one interactive owner login through Pi's own Anthropic OAuth flow (copy-code by
  default, `PI_PROBE_CLAUDE_LOGIN_METHOD=browser` for the loopback callback). Makes no model request.
- `probe.mjs`: resolves the route through `ModelRuntime`, runs the two requests through the guard and
  writes `output/claude-<mode>-result.json`. Offline mode uses `fake-anthropic.mjs` and always reports
  `NOT RUN`; only live mode can report GO.

## Commands (from the worktree root)

```bash
bash tools/pi-probe/claude/run.sh offline          # frozen install, tests, offline probe
PI_PROBE_CLAUDE_AUTHORIZATION=<owner comment timestamp> bash tools/pi-probe/claude/run.sh login
PI_PROBE_CLAUDE_AUTHORIZATION=<owner comment timestamp> bash tools/pi-probe/claude/run.sh live
```

`login` needs a terminal. `live` stops with a stable code (`login_required`, `live_call_budget_exhausted`,
...) rather than guessing; delete nothing from `output/` to obtain more budget.
