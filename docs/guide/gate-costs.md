# Gate cost policy

Tests are budgeted by cost and judged by defect-catching value, not count. Never relax a
failing test to go green. The placement/reviewer rules live in `AGENTS.md` and the shipped
worker/reviewer contracts. Test-cost justification belongs in the candidate's change report.

## Fixed tiers and limits

| Tier | Membership | When | Test budget / guard |
|---|---|---|---:|
| quick | Touched modules/consumers, crate-wide fallback; declared tests replace selection | Each review round / Plan member | 180s test execution |
| full | Workspace except the fixed torture catalog; restore affected torture suites when an owning module changes | Main landing | 600s test execution |
| slow | Fixed race/crash/torture/restart catalog in `profile.slow` | Nightly and owning-module changes | 1800s nightly maintenance wall guard |

Plan verification keeps every suite and both optional lanes. Missing/older context is also
conservative. `gate_lanes.py` maps the slow catalog to owning paths; unmapped catalog entries
run, never silently disappear. Shared dependency/store/Git/herdr changes restore all torture
coverage. Quick's explicit declared tests can opt into slow tests.

`.config/gate.toml` enforces cumulative **test execution**, not compilation: nextest, the
three work budgets, and Bun tests share one per-attempt ledger. Build/preparation and
whole-recipe wall time are reported separately. The timer logs load/core before/after and
monitors it during tests; a phase that observes load/core >1.0 or an unavailable reading
is reported as busy-exempt, not charged to the quiet-host cap. Tests still run, and assertion
failures still fail. This owner-approved policy prevents cold-cache or host-contention
budget flakes; a busy-exempt run is not proof of meeting the quiet-host timing target.

Nextest compiles candidate binaries once, then lists/runs them using fresh cargo/binaries
metadata. The measured run cannot compile source again. Pi cross-lane Rust helpers are
prebuilt before Bun timing; dependency install and typecheck are reported as preparation.
Quick Clippy checks touched production targets; nextest still compiles/runs selected tests.
Full retains all-target Clippy, so test-body lint coverage moves to landing, not deletion.
Doctest/build/docs-check step walls remain separately visible in the log. On budget expiry
the timer terminates its owned process group and exits nonzero.
Linked worktrees keep Cargo's local target by default. Forced primary-target sharing was
tried and reverted: it did not solve the cold build and concurrent candidates can overwrite
the stable top-level CLI path after the build lock is released. Explicit `CARGO_TARGET_DIR`
is respected; callers sharing executable targets must serialize through execution. The
docs checker always uses the candidate binary from the actual target, not another path. Work budgets keep the workspace's `corral-core/test-clock` feature union
to avoid recompiling core/ops/TUI merely to toggle a test-only feature. An experimental
Clippy `--profile test` alignment made the cold check slower and was reverted; the time
test assertions and overflow checks were not relaxed. It never
changes a test's result to PASS. `gate_case.py` enforces ordinary 10s guards with the same
observed-load rule, using nextest's binary/test identity and candidate metadata. Nextest itself
reports at 10s without unconditional termination: its native wall guard cannot distinguish
a busy host. Nextest evaluates the exception filters (including ignored work matrices);
the runner never reimplements that DSL. Over-budget observations retain load/exit evidence,
and cancellation forwards to the owned child group. No retries. Explicit
measured integration exceptions retain their original 30s report-only guards, each with a
one-line defect/fixture reason in `.config/nextest.toml`; this does not exclude them from
full. The owner approved these exceptions rather than dropping useful journey coverage.
Do not add an exception merely because a test is failing; diagnose it and justify placement.

Each gate prints candidate-added test registrations (Rust, Bun, Python), measured added suite
seconds and unmeasured additions. Renames and case splits count as additions and need their
cost explained too. Rust/Bun use the current attempt's receipt log; Python can record its
containing-script time via `scripts/test_cost.py python-tests <scripts...>` (a conservative
suite upper bound). Unselected/slow/ignored tests are **unmeasured**, not free. More than 30s
measured added suite time fails without a `Test-cost justification:` line added to a candidate
Markdown report. The reviewer checks the actual defect-catching reason, including unmeasured
additions; the line alone is not semantic approval.

## Optional lanes

- Pi: package or shared agent/inbox/CLI/capability contracts. Full runs all Bun tests; quick
  follows local TS imports to test consumers, with all-package fallback for unknown inputs,
  manifests and Rust wire contracts. Worker/reviewer instruction assets select their role
  contract consumers. Typecheck always runs when the Pi lane runs.
- Darwin: Rust platform/cfg code, build scripts or Cargo dependencies/toolchain changes.
  Additions and removals count; editing within an existing cfg file counts too. Python/docs
  strings mentioning `target_os` do not trigger a cross-check. Installed targets are reused,
  never downloaded during a gate.
- Plan verify always includes both, independently of changed paths. Its context carries
  `plan_verify: true`; ordinary task gates carry false. Older contexts without the field run
  conservatively, not as though they had opted out.

## Host nightly slow lane

`scripts/slow_lane.py` is a host-invoked maintenance lane, **not another worker gate**. It:

1. Sets its own nice priority to 19 and I/O class to idle; descendants inherit both.
2. Mirrors Corral's machine-load rule (`load1 / available cores <= 1.0`), respecting affinity
   and conservatively rounding cgroup-v2 quotas down. Unavailable readings fail closed.
   Busy hosts defer with exit 75 before starting herdr, tests, or board writes.
3. Takes a nonblocking local lock and refuses a dirty worktree.
4. Starts a fresh plugin-free private HOME/XDG/server through `test-herdr.sh`, runs only the
   explicit nextest slow profile under a quiet-load-only maintenance wall guard, and lets the helper clean up its own
   server/directories. It never reuses or stops an owner's server.
5. Retains raw logs locally and as `log` artifacts with tier `keep` on a monitoring task.
   Each distinct failing test gets one open task keyed by binary+test; subsequent failures
   reuse that task and attach proof. A build/startup failure gets one infrastructure task.
   These are Backlog, origin agent, never dispatched. Board operations happen **outside**
   the private test environment. A reporting error propagates with the local proof retained;
   no mutation is blindly retried after a lost acknowledgement.

The PM installs a cron entry **after landing**, choosing the monitoring task and host paths.
No worker should install a crontab or run an unauthorized slow tier to validate this setup.
Example for this host (03:15 local time):

```cron
15 3 * * * cd /home/pawis/miti-workspace/corral && mkdir -p .evidence && nice -n 19 ionice -c 3 /usr/bin/python3 scripts/slow_lane.py --root /home/pawis/miti-workspace --corral /home/pawis/.cargo/bin/corral --report-task T-761 >>.evidence/slow-cron.log 2>&1
```

No GitHub schedule is installed. An owning-module full-gate failure already returns to its
candidate task/worker with the receipt; the nightly lane additionally files deduplicated
root-cause tasks for any slow failures it observes. No retries-to-green in either path.
Mutation sampling and monthly gardening/flake triage are separate follow-up work, not extra
per-gate checks.
