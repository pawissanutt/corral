# Claude cache efficiency: offline measurements

Scope: improve the existing one-shot adapter, then assess resident sessions without provider
requests or owner credentials. Integration base: `97d6cceefdfb76997a7c1fc21c39408f462bdda0`.

## Changes

- Pi explicitly owns compaction. Both coding/search SDK queries disable native auto-compaction;
  unexpected compact boundaries fail instead of silently changing history. Incomplete coding
  streams cannot report success or commit cache metadata.
- Replay seals user/assistant messages and parallel text-result batches into stable text blocks.
  Tool names/IDs, text, thinking-as-history and images remain represented. Moving question state
  follows cached history rather than changing the root system prompt. Authority rules are unchanged.
- The single history marker retains the successful-prefix digest and four-marker SDK budget.
  Large image batches retain the conservative rolling fallback. Expired and changed prefixes are
  revalidated; late completions cannot overwrite a newer confirmed frontier.
- Cache retention is configurable (`CORRAL_CLAUDE_CACHE_TTL=5m|1h|none`, default `5m`), with explicit
  Pi request policy taking precedence. SDK and replay lifetimes agree. Summaries can disable caching.
  Reported one-hour write counts keep their pricing tier; failed results retain reported usage.
- `/cache-stats` distinguishes latest-request and weighted whole-session input cache rates.
  Sanitized `claude_cache` diagnostics retain lifetime/frontier/tail/timing and reported usage,
  never prompt bodies or credentials. Local prefix matching is not a server cache-hit claim.

## Replay comparison

From `packages/corral-pi`:

```sh
bun test/support/claude-cache-benchmark.ts 97d6cceefdfb76997a7c1fc21c39408f462bdda0
```

The script loads the exact base renderer/cache from Git and compares them with the candidate.
Each scenario has six bounded requests and uses an in-memory model of the 20-block cache lookup.
Numbers below count **serialized replay JSON bytes**, including cold requests. They are not
provider tokens or measured cache hits, and exclude SDK tools/system, TTL and server behavior.

| Scenario | Modeled read % before → after | Uncached historical tail bytes before → after | Worst lag blocks before → after |
|---|---:|---:|---:|
| Serial text | 78.51 → 78.62 | 0 → 0 | 0 → 0 |
| 25 parallel text results | 45.78 → 72.69 | 161,351 → 0 | 165 → 0 |
| 25 parallel image results | 37.71 → 45.14 | 244,655 → 181,622 | 416 → 170 |
| Compaction halfway | 41.63 → 51.60 | 65,224 → 0 | 66 → 0 |
| Local cache tracker restarted halfway | 52.50 → 72.69 | 65,224 → 0 | 66 → 0 |

Interpretation: text-batch lag is removed in this workload, while image-heavy requests retain a
known limit. Compaction intentionally introduces a different prefix, so a cold request is expected.
The restart case retains the synthetic server cache but recreates local bookkeeping; it does not
claim that an SDK resident session can safely resume after a crash.

## Real SDK/CLI boundary and resident timing probe

```sh
CORRAL_TEST_CLAUDE_RESIDENT_PROBE=1 bun test test/claude-wire-cache.test.ts
```

This runs the real SDK and Claude Code `2.1.306` in private network/PID namespaces against a
synthetic loopback endpoint. It validates Git-state stability, growing-history prefix reuse,
`5m`/`1h`/disabled wire policy and the four-marker budget. The normal gate runs seven synthetic
requests. The optional timing probe adds six; none reach Anthropic or use Claude quota.

One observed probe, three synthetic requests per mode:

| Mode | SDK queries / engine launches | Wall time |
|---|---:|---:|
| One-shot | 3 | 4,980 ms |
| Resident streaming-input probe | 1 | 1,882 ms |

This is **process/query overhead only** on a shared host, with no actual model inference or
provider cache. It is one observation, not a throughput guarantee or evidence of production
continuity. Timing is report-only and is not a test assertion. The optional probe is not added
to the normal gate's request count.

## Assessment of resident sessions

Resident sessions remain promising: this observation saved about 3.1 seconds across three tiny
requests, and large image batches still expose replay limits. But most of the demonstrated text
cache-boundary waste is fixable without introducing resident state. This change therefore keeps
the production provider one-shot. A future resident implementation still needs verified committed
history, compaction/tree invalidation, orphan-prompt recovery after abort/restart, account/model/
tool/settings compatibility, per-session serialization and result-to-turn correlation. It must
not bypass Pi's tool execution, permission dialogs or board authority.

No live provider cache-hit rate, subscription consumption or end-to-end coding speed was measured.
