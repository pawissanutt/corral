# T-956 — SDK-free Darwin checks with ring 0.17.14

Historical record of the original Darwin-only candidate `254175e7`. The owner later
authorized the exact T-955 fixture consolidation; see the [combined-scope record](T-956-consolidation.md).
Checks and approval of the original candidate are not approval of that combined candidate.

## Cause, not attribution

The original full-landing failure is retained unchanged at
`T-955:R1:dcb4a5bd-T-955-R1-2335527-1794671834455854458.log`, and copied to
`T-956:R1:dcb4a5bd-T-955-original-darwin-failure.log`.
Its SHA-256 is `dcb4a5bdb59fcd124ebc3a1ee0e2958d91953bb6c59505388c911b9bd0986771`.
Lines 249–252 identify ring's failed build script; lines 421–426 show Linux `cc`
receiving `-arch arm64`, `-mmacosx-version-min=11.0` and `-gfull`.

Bounded probes used exported, unmodified Cargo/crate snapshots inside this task's
ignored `.evidence/`, with private targets, two compilation jobs and offline/locked Cargo:

```sh
LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo check --offline --locked -p ring \
  --target aarch64-apple-darwin \
  --config 'target.aarch64-apple-darwin.zstd.rustc-link-lib=[]'
```

Both T-955's candidate `d6e12f7023328bcd1155901f959463bd020ca322` and its base
`8ddbe80ac59f5d344cfb4f02fe1a73bda63c5294` exit 101 with the same rejected Apple
compiler flags. Their Cargo.lock blob is identical (`6bf413e76821f89d13e09e6d56a443774b849a2f`),
as is gate.sh (`21c1abdbb540342f37efa1d35a0c9007914c37b0`). T-955 changes only the
assistant-entry fixture. This is a pre-existing native cross-compiler preparation
failure, not evidence against that fixture. The dependency path is
`corral -> corral-daemon -> snow 0.10.0 -> ring 0.17.14`.
Probe artifacts: `T-956:R1:32fdb9d4-probe.log` (candidate),
`T-956:R1:fde91cc2-probe.log` (base).

## Small repair and coverage argument

Only the existing Darwin **cargo check** commands gain:

```text
--config target.<triple>.ring_core_0_17_14_.rustc-link-lib=[]
```

This uses Cargo's supported [links build-script override](https://doc.rust-lang.org/cargo/reference/build-scripts.html#overriding-build-scripts),
like the existing zstd override. This preserves T-746's check-only boundary
(`T-746:R1:a6380134-T-746-report.md`). It skips native preparation, not Rust compilation.
It supplies neither fake crypto nor alternate Rust code. Packages, features,
`--workspace --all-targets`, target triples, installed-target discovery and error
propagation remain unchanged.

Audit of the exact registry source pinned by Cargo.lock (`ring 0.17.14`, checksum
`a4689e6c2294d81e88dc6261c768b63bc4fcdb852be6d1352498b114f61383b7`):

- `Cargo.toml:17–18` declares `links = "ring_core_0_17_14_"` and `build.rs`.
- `build.rs:268–365` validates that versioned name and selects native C/assembly inputs.
  Its generated files are native headers/assembly, not Rust modules.
- `build.rs:479–504,536–548` builds core/test native archives and emits their link
  search/library directives. `cc`'s automatic Cargo metadata is disabled at line 537.
  The other Cargo emissions are rerun directives. There are no `rustc-cfg`,
  `rustc-env`, or downstream metadata outputs to reproduce.
- Rust sources have no `OUT_DIR`/generated-source include dependency.
  `src/prefixed.rs:1–15` constructs symbol prefixes from Cargo's ordinary package
  environment, which remains available without a build script. Rust's normal
  architecture/OS/feature cfgs still select its FFI declarations and implementations;
  no no-assembly/docs cfg is introduced.

Therefore omitting only the native link directives preserves the Rust type-check
inputs for this pinned version. The skipped version assertion has been checked
against the pinned manifest. Re-audit the contract on dependency upgrades; the
versioned override does not match a different ring links key.

SQLite's existing linked path still copies its bundled Rust bindings; zstd's
checked-in bindings still need no generated Rust. Overrides are command arguments
and a command-local SQLite assignment, never persisted Cargo settings or exported
build/test environment. Real builds still compile/link native code normally.
This is equivalent **Rust/cfg checking**, not Apple C/assembly or ABI validation.

## Verification

Host: Linux x86_64, rustc/cargo 1.94.1; both Darwin standard libraries already installed.
Nothing was installed, downloaded, uninstalled or provisioned. For **each** of
`aarch64-apple-darwin` and `x86_64-apple-darwin`:

```sh
LIBSQLITE3_SYS_USE_PKG_CONFIG=1 CARGO_BUILD_JOBS=4 cargo check --offline --locked \
  --workspace --all-targets --target "$target" \
  --config "target.$target.zstd.rustc-link-lib=[]" \
  --config "target.$target.ring_core_0_17_14_.rustc-link-lib=[]"
```

Both passed on the complete current dependency graph (89s / 92s preparation;
existing platform-unused warnings, no errors). Logs:
`T-956:R1:c6f82b01-workspace-aarch64-apple-darwin.log`,
`T-956:R1:b90dfc51-workspace-x86_64-apple-darwin.log`.

- Extended the existing `test_darwin_native_c_bypass_is_check_only` regression.
  It fails before the gate change (`75d2e6b6-regression-red.log`). All three Darwin
  fixture tests then pass in 8.261s (`dcc59d7d-darwin-fixtures-green.log`), retaining
  installed-target coverage, exact check argv, no overrides on native commands,
  and nonzero failure propagation. These are fake-command orchestration fixtures,
  not a manually run full gate. Added test registrations: **0**; only existing
  assertions/expected argv changed, with negligible added suite runtime.
- Native `CARGO_BUILD_JOBS=4 cargo check --offline --locked -p corral-daemon --all-targets`
  passes with no overrides (`b7d09ff8-native-daemon-check.log`).
- Native ring build/link/crypto smoke passes with no overrides
  (`7df0f986-native-ring-smoke.log`). A disposable crate pins ring 0.17.14 and checks
  SHA-256(`abc`) against `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`.
  Its manifest/source are attached as `a012056d-Cargo.toml` / `83699b95-main.rs`.
  Running ring's registry unit test directly was refused by Cargo because a
  non-workspace package needs dev-dependencies; the smoke avoids altering the
  workspace or installing those dependencies. Scratch Rust LSP diagnostics timed
  out indexing; the successful real compilation/execution is the fallback check.
- `bash -n scripts/gate.sh` and `git diff --check` pass. No LSP is configured for
  shell/Python. Ruff reports the same pre-existing F402 at test_gate.py:398 on both
  candidate and base (`28434196-changed-file-checks.log`); it was not suppressed.
- The broader `python3 scripts/test_gate.py` runs 38 tests, with five failing
  nextest subcases in two unrelated tests (`1fffecee-gate-fixtures-green.log`).
  A bounded rerun of those two tests on unchanged base reproduces all five
  (`fb0c9c27-baseline-fixture-failures.log`): exit-code/execute-bookkeeping expectations,
  including 1 vs 100/4/23. PM follow-up requested via T-956/R1/Q1; no assertions weakened.

## Boundaries and remaining validation

The required receipted quick gate follows commit/rebase; its result/log is recorded
separately by corral. The normal independent review and full landing gate remain
required; no other gate/tier was invoked. Actual macOS native compilation/linking
requires an Apple toolchain and was not attempted. This repair needs no SDK because
it restores only the lane's existing Rust-only contract.

No production Rust, Cargo manifests/lockfile/config, features, transport, targets,
T-955/T-831 fixes, task/review history or main-checkout files were changed.
Main's tracked deletions and the PM's preservation/landing decisions are untouched.
