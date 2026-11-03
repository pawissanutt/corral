# Gate cost policy

Tests are budgeted by cost and judged by defect-catching value, not count. Never relax a
failing test to go green. The placement/reviewer rules live in `AGENTS.md` and the shipped
worker/reviewer contracts. Test-cost justification belongs in the candidate's change report.

## Fixed tiers and report-only timing references

| Tier | Membership | When | Timing reference (report-only) |
|---|---|---|---:|
| quick | Touched modules/consumers, crate-wide fallback; declared tests replace selection | Each review round / Plan member | 180s test execution |
| full | Workspace except the fixed torture catalog; restore affected torture suites when an owning module changes | Main landing | 600s test execution |
| slow | Fixed race/crash/torture/restart catalog in `profile.slow` | Nightly and owning-module changes | 1800s nightly maintenance wall observation |

Plan verification keeps every suite and both optional lanes. Missing/older context is also
conservative. `gate_lanes.py` maps the slow catalog to owning paths; unmapped catalog entries
run, never silently disappear. Shared dependency/store/Git/herdr changes restore all torture
coverage. Quick's explicit declared tests can opt into slow tests.

**Owner's relaxed T-761 scope:** every time threshold is report-only. Enforcement belongs
to **T-776**, not this candidate. The per-test10s reference, quick180s/full600s runtime
references, quiet-load accounting and added-suite30s reference print observations and
never replace a command's exit status. Assertions, compilation, lint and actual command
failures still fail normally; there are no retries-to-green.

`.config/gate.toml` records cumulative nextest/work-budget/Bun execution separately from
build/preparation and total wall. Load/core is sampled before, during and after execution;
busy/unknown phases remain identified, not timing qualification. No timer terminates a
command for slowness. Cancellation still forwards to owned child groups.

Nextest compiles candidate binaries once and reuses fresh metadata. Pi cross-lane Rust
helpers are prebuilt before Bun timing; install/typecheck are preparation. Quick Clippy
checks touched production targets; full retains all-target Clippy. Work budgets preserve
the core/test-clock feature union. Linked worktrees keep local targets unless explicitly
overridden, avoiding cross-candidate top-level CLI overwrites; docs use the actual target.
The slower Clippy test-profile experiment was reverted.

`gate_case.py` reports per-test observations and candidate body/attribute changes against
the integration base. The log and `.evidence/<attempt>-existing-test-costs.md` retain the
T-766 gardening input; attach that report without changing the committed candidate. Native
nextest reports at10s; documented integrations retain their pre-existing30s report-only
guards, with reasons. These are not additional full-tier exclusions.

The gate prints added Rust/Bun/Python registrations, measured suite seconds and explicit
unmeasured additions. Renames/case splits count as additions. Python containing-suite time
is a conservative upper bound. More than30s without a `Test-cost justification:` is a
report, not an automated failure here. Worker/reviewer placement and justification guidance
remains in force; reviewers evaluate named defect-catching value, not evidence volume.

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
   explicit nextest slow profile with a reported maintenance wall threshold, and lets the helper clean up its own
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
