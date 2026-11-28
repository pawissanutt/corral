# Gate cost policy

Tests are budgeted by cost and judged by defect-catching value, not count. Never relax a
failing test to go green. The placement rules live in this repository's `AGENTS.md`; the
reviewer rules also ship in the review contract (`review-contracts/common.md`). The Pi role
contracts and the worker skill carry no test policy. Test-cost justification belongs in the
candidate's change report.

Time-based fixtures must use the recorded activity's clock, not a fresh raw wall-clock
reading: Corral's ordered timestamps can stay ahead after a host clock steps backwards.
For idle-GC fixtures, age every activity input, including session and scratch creation,
not just the session end and HEAD commit.

## Quarantined tests

`.config/gate-quarantine.toml` lists exact nextest identities, their fixing tasks and reasons.
Every gate that runs Rust tests runs them in one separate nextest invocation after the ordinary
selection, at the profile's usual parallelism and without stopping at the first failure, and
reports each test's pass or failure from that run's JUnit report. Only ordinary test failures
and quarantine infrastructure errors (the run not completing, or a listed identity that did
not run) fail the gate. A quick gate whose changed paths touch no crate and declare no tests
builds no test binaries and runs neither the selection nor the quarantine. The fixing task
must remove its own one-line entry when its fix lands.

## Fixed tiers and report-only timing references

| Tier | Membership | When | Timing reference (report-only) |
|---|---|---|---:|
| quick | Touched modules/consumers, crate-wide fallback; declared tests replace selection | Each review round / Plan member | 180s test execution |
| full | Workspace except the fixed torture catalog; restore affected torture suites when an owning module changes | Main landing | 600s test execution |
| slow | Fixed race/crash/torture/restart catalog in `profile.slow`, plus the Bun catalog `PI_SLOW` in `scripts/gate_lanes.py` | Nightly and owning-module changes | 1800s nightly maintenance wall observation |

Plan verification keeps every suite and both optional lanes. Missing/older context is also
conservative. `gate_lanes.py` maps the slow catalog to owning paths; unmapped catalog entries
run, never silently disappear. Its Bun catalog `PI_SLOW` lists each slow Pi test with its reason
(currently `a cancelled call stops its browser and removes its profile`, "load-sensitive
live browser cancellation") and `PI_SLOW_OWNERS` (the browser extension, its test file, and the `exits.ts` and `wait.ts` test
support files it waits through). The
Pi lane passes `CORRAL_PI_SLOW` (0 in quick; in full, 1 only when an owner changed; 1 in plan
verify), and a catalog test skips when it is 0. Shared dependency/store/Git/herdr changes restore all torture
coverage. Quick's explicit declared tests can opt into slow tests.

Every time threshold is report-only. The per-test10s reference, quick180s/full600s runtime
references, quiet-load accounting and added-suite30s reference print observations and
never replace a command's exit status. Assertions, compilation, lint and actual command
failures still fail normally; there are no retries-to-green.

`.config/gate.toml` records cumulative nextest and Bun execution separately from
build/preparation and total wall. Load/core is sampled before, during and after execution;
busy/unknown phases remain identified, not timing qualification. No timer terminates a
command for slowness. Cancellation still forwards to owned child groups.

Nextest compiles candidate binaries once and reuses fresh metadata. Pi cross-lane Rust
helpers are prebuilt before Bun timing; install/typecheck are preparation. Quick Clippy
checks touched production targets; full retains all-target Clippy. Linked worktrees keep
local targets unless explicitly overridden, avoiding cross-candidate CLI overwrites.
The slower Clippy test-profile experiment was reverted.

After each nextest run, `gate_case.py` reads its JUnit report and records per-test
observations and candidate body/attribute changes against the integration base; no wrapper
process runs per test. Load/core comes from the once-per-second samples the test phase
records. The log and `.evidence/<attempt>-existing-test-costs.md` retain the
timing data for test maintenance; attach that report without changing the committed candidate. Native
nextest reports at10s; documented integrations retain their pre-existing30s report-only
guards, with reasons. These are not additional full-tier exclusions.

The gate prints added Rust/Bun/Python registrations, measured suite seconds and explicit
unmeasured additions. Renames/case splits count as additions. Python containing-suite time
is a conservative upper bound. More than30s without a `Test-cost justification:` is a
report, not an automated failure here. Worker/reviewer placement and justification guidance
remains in force; reviewers evaluate named defect-catching value, not evidence volume.

## Repair commands

When a gate's nextest run fails, `scripts/gate_repair.py` reads that run's JUnit report and
prints, just before `FAILED:`, one command per failing test binary. Each builds only that
binary's target and runs only its failed tests under a private test herdr. Autopilot carries
the block into the send-back with one instruction: rerun those commands after each edit until
they pass, then gate the candidate once as the send-back says, not once per edit. The commands
are diagnostics, never a gate result: the gate still passes or fails on its own commands, a
stale or missing report prints nothing, and compile, lint and Pi failures print no block.

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

### SDK-free Darwin Rust checks

Each installed Darwin target is checked with `cargo check --workspace --all-targets`.
This is Rust/cfg coverage, not native compilation or linking. Only these invocations use
`LIBSQLITE3_SYS_USE_PKG_CONFIG=1` and command-line Cargo `links` overrides:

| Native crate (`links` key) | Rust-only contract |
| --- | --- |
| SQLite (environment variable, not a build-script override) | Its linked build path still copies bundled Rust bindings into `OUT_DIR`; replacing the build script would lose those bindings. |
| zstd-sys (`zstd`), libz-sys 1.1.33 (`z`), libnghttp2-sys 0.1.13 (`nghttp2`) | Checked-in Rust bindings; generated headers/pkg-config files are C-only. |
| ring 0.17.14 (`ring_core_0_17_14_`) | Only native link directives; no generated Rust or Rust cfg/env inputs. |
| openssl-sys 0.9.118 (`openssl`) | Handwritten bindings with the current features (no `bindgen`, BoringSSL or AWS-LC). Supply `openssl`, the cumulative `ossl*` cfgs through `ossl350`, and matching `version_number`/`conf` metadata. |
| curl-sys 0.4.91+curl-8.22.0 (`curl`) | Checked-in bindings; preserve `libcurl_vendored`, `link_libnghttp2`, `link_libz`, `link_openssl` cfgs and `static=1` metadata, matching the resolved static-curl/http2/ssl features. |

The OpenSSL profile is system-installed **OpenSSL 3.5.0 LTS**, version number
`30500000` (hex), with SSLv3 methods disabled (`OPENSSL_NO_SSL3_METHOD`) and the other
detected APIs enabled. This is a supported macOS OpenSSL installation, not Apple's
LibreSSL or a Linux library probe: the current dependency tree uses system OpenSSL
(no `openssl-src`/vendored feature), and 3.5 provides a maintained LTS API profile.
The `rustc-cfg` list reproduces `openssl-sys`'s `build/cfgs.rs` for that version,
including its `osslconf` value. `DEP_OPENSSL_VERSION_NUMBER` and `DEP_OPENSSL_CONF`
let `openssl` 0.10.82 derive the same API cfgs; LibreSSL/BoringSSL/AWS-LC metadata
must remain absent. `curl` 0.4.51 reads the version and `DEP_CURL_STATIC`, retaining
certificate probing without legacy OpenSSL initialization. `isahc` 1.8.3's build
script only exports its own enabled features and reads no `DEP_CURL_*` metadata.

Do not invent include/root paths: `DEP_Z_INCLUDE`, `DEP_NGHTTP2_ROOT` and
`DEP_OPENSSL_INCLUDE` are only used to compile curl's C sources, whose script is
also overridden. No current Rust consumer needs those headers or curl's generated
C headers/pkg-config files. None of the overridden crates requires generated Rust
in this feature profile; SQLite is deliberately the exception handled by its own
build script.

No Apple SDK is needed for these checks. Re-audit the resolved features, generated
files and cfg/metadata consumers on native dependency upgrades, especially the
versioned ring `links` key, the OpenSSL version/cfg/conf profile, and curl's
static/http2/ssl contract. Never persist these overrides in Cargo config or use
them for builds/tests; real builds still require the platform's native toolchain.

## Host nightly slow lane

`scripts/slow_lane.py` is a host-invoked maintenance lane, **not another worker gate**. It:

1. Sets its own nice priority to 19 and I/O class to idle; descendants inherit both.
2. Mirrors Corral's machine-load rule (`load1 / available cores <= 1.0`), respecting affinity
   and conservatively rounding cgroup-v2 quotas down. Unavailable readings fail closed.
   Busy hosts defer with exit 75 before starting herdr, tests, or board writes.
3. Takes a nonblocking local lock and refuses a dirty worktree.
4. Starts a fresh plugin-free private HOME/XDG/server through `test-herdr.sh`, runs only the
   explicit nextest slow profile and then the Bun `PI_SLOW` catalog (with `CORRAL_PI_SLOW=1`,
   whatever nextest's result) with a reported maintenance wall threshold, and lets the helper clean up its own
   server/directories. It never reuses or stops an owner's server.
5. Retains raw logs in `.evidence/slow` for the host operator. It does not file board tasks
   or attach artifacts.

Run `python3 scripts/slow_lane.py` without arguments from the repository root. The JSON
summary includes the candidate, exit code, log path and failures. An empty Rust slow profile
is allowed; the Bun catalog still runs and its failures propagate.

No GitHub schedule is installed. Only the host operator should schedule this lane;
workers should not install a crontab or run an unauthorized slow tier. An owning-module
full-gate failure already returns to its candidate task/worker with the receipt.
No retries-to-green in either path.
Mutation sampling and monthly test maintenance and flake triage are not per-gate checks.
