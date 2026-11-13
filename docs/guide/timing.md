# Timing corral waits

Timing records attribute wall time to file locks, SQLite writes and busy waits,
herdr, git and named read/derive spans. Enable recording for a workspace:

```sh
touch "$(dirname "$(corral daemon socket)")/timing-on"
```

Disable it with `rm "$(dirname "$(corral daemon socket)")/timing-on"`. No agent
restart is needed: the switch is checked at command exit and after every daemon
pass. Without the file, commands write nothing and their output is unchanged.
Records are appended to `timing.jsonl` beside the daemon socket, normally under
`$XDG_RUNTIME_DIR/corral/<store-id>/`. The log does not survive a reboot.
Each line is one JSON object: one `process` record for any corral process that
resolved a store, plus one `pass` record per daemon pass. The first store opened
by a process determines its process-record destination. A missing runtime
directory means recording is off; timing does not create it to check the switch.

Appending stops silently when the file is larger than 16 MiB. Remove or rotate
`timing.jsonl` to resume. Recording failures never fail commands. Each line is
under 4 KiB and appended with a single write; a record exceeding that limit is
omitted. No task content, titles, argument values or free text is recorded.

## Fields

| Field | Meaning |
| --- | --- |
| `ts` | RFC 3339 timestamp when the record was assembled. |
| `pid` | Corral process ID. |
| `kind` | `process` or `pass`. |
| `verb` | Up to two parser-resolved command names, such as `pm brief` or `gate`; passes use `daemon pass`. |
| `wall_ms` | Elapsed wall time of the process or pass. |
| `cpu_ms` | User plus system CPU time across process threads, or `null` if unavailable. |
| `max_rss_kb` | Process peak resident memory in KiB (`VmHWM` on Linux), otherwise `null`. |
| `lock_wait_ms` | Time inside blocking store locks and the store-gate acquisition retry loop. |
| `lock_waits` | Blocking lock acquisitions plus nonblocking lock attempts. |
| `lock_try_failed` | Failed nonblocking store-lock attempts. |
| `sqlite_busy_ms` | Actual time spent sleeping in the SQLite busy handler. |
| `sqlite_busy_waits` | Number of sleeps in the busy handler. |
| `sqlite_apply_ms` | Wall time of SQLite apply calls, including writer and busy waits. |
| `sqlite_applies` | Number of SQLite apply calls. |
| `herdr_ms` | Wall time of herdr subprocess attempts, including version checks. |
| `herdr_calls` | Number of measured herdr subprocess attempts. |
| `git_ms` | Wall time of git subprocess attempts, including streamed blobs. |
| `git_calls` | Number of measured git subprocess attempts. |
| `spans` | Static span names mapped to `{ "n": invocation count, "ms": total wall time }`. |

Pass counters, spans and CPU time cover the interval from the previous pass's
end-snapshot to the current pass's end-snapshot (the first starts at loop entry).
This includes reader and other daemon-thread work between passes; `wall_ms`
remains the current pass's own wall time. Peak memory remains the process-wide
high-water mark. Process records are lifetime totals. CPU time reuses corral's
best-effort process CPU reader (Linux `/proc/self/stat`). Counter categories and
nested spans overlap, so summing all fields does not produce total wall time.

Instrument coarse read/derive work with `let _span = corral_core::timing::span("static.name");`.
The guard adds process-wide count and elapsed wall time on drop; repeated and nested names accumulate independently.

Sum process lock wait time by verb without double-counting daemon pass records:

```sh
jq -s 'map(select(.kind == "process"))
  | group_by(.verb)
  | map({verb: .[0].verb, lock_wait_ms: (map(.lock_wait_ms) | add)})' "$(dirname "$(corral daemon socket)")/timing.jsonl"
```
