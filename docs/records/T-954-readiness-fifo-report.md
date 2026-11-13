# T-954: duplicate readiness FIFO owners

## Outcome and limits

The fake-herdr fixture has a deterministically reproduced check-then-create race.
Two workspace-list callers can both observe an absent `.sent` marker. After the
one-shot reader consumes the first byte and closes, a late second writer blocks
opening the FIFO. The correction uses atomic `mkdir` ownership, so only its winner
opens the FIFO. No production code or Rust assertions changed.

This establishes a fixture-owned defect and a daemon stall path. **It does not
establish that the historical T-935 failure took this interleaving.** That run did
not retain its child syscall/process state. A green rerun cannot supply that proof.
Normal independent review and the integrated full landing gate remain required.

## Revisions and historical evidence

- Failed T-935 candidate: `3346270db3520eea647817d6d38800e0a424b3a3`.
- Its comparison base: `d10ded29`.
- T-954 starting main: `7c43fe4b17e29dd7d6c2f72221891eb9fee6620c`.
  This main includes later daemon work; it is not the failed candidate.
- The historical fixture and `version_skew_cli.rs` are byte-identical to the
  T-954 starting versions. The candidate/base diff has no changes in these files,
  `corral-daemon`, `corral-herdr`, or the relevant sync/health caller files.
- Raw failure retained unchanged as `historical-failure.log`, copied using the
  read-only artifact command from
  `T-935:R1:36e8a2b2-T-935-R1-2064625-1794540644393824542.log`.
  SHA-256: `36e8a2b203b8ac5d25a88adee7c850af6d927f328f532b65962249cfffa918cf`.
  Its lines 4071–4088 record 120.488s and the notice receiver timing out at
  `version_skew_cli.rs:85`. The earlier readiness wait and executable rename had
  already returned. No evidence was inferred from the host's load average.

## Candidate execution path

The following references are to **3346270d**, inspected with `git show`, not to
unidentified current source:

1. `crates/corral-daemon/src/lib.rs:121–126` starts background health sampling,
   then runs synchronous passes; the runtime-latched exit decision follows the pass.
2. `crates/corral-daemon/src/health.rs:187–218` immediately starts a health thread;
   its `ctx.herdr.available()` call overlaps main-thread sync.
3. `crates/corral-ops/src/sync.rs:135–137,253,269` checks runtime before reaching
   its own `ctx.herdr.available()`. A call already in progress when replacement
   occurs must finish before sync can reach a later runtime check.
4. `crates/corral-herdr/src/lib.rs:149–151,620–627`: `available()` invokes
   `workspace list`; `raw()` uses synchronous `Command.output()` with null stdin.
   A fake child stuck opening the FIFO therefore holds that caller indefinitely.
5. `crates/corral-daemon/src/lib.rs:178–241` waits for sync before completing its
   pass and draining diagnostics; `Exit::Upgrade` maps to 75 at lines 33–38.

Thus a health winner and late sync loser can explain a missing notice after
replacement. If health alone is the blocked loser, this argument does **not**
prove the main sync loop stalls. The controlled reproduction labels roles but
runs the fixture directly, not an instrumented daemon.

## Deterministic failing-first probe

Attached `fifo_regression.py` is a bounded, disposable fixture-level regression.
It sources the byte-identical shell file and wraps the ownership command only to
pause **after its real result**. Two callers are alive concurrently. On the old
fixture both absence checks return zero before either marker creation is allowed.
On the fixed fixture the real `mkdir` results are zero and one. A pipe barrier,
not a sleep or host contention, controls release order:

1. Hold both callers after their ownership operation.
2. Release the first caller to a real FIFO reader; require one byte and reader exit.
3. Release the second caller only after the reader has closed.
4. Require that caller to return the exact workspace-list JSON without another reader.

Bash is used to source the unchanged POSIX fixture because dash does not allow a
function named `[`; this is an explicit probe limitation, not a claim to reproduce
the historical daemon process. The corrected fixture is separately run by its real
`/bin/sh` interpreter in the existing real-daemon tests below.

| Probe | Result |
| --- | --- |
| Historical candidate fixture | RED: claim results `[0,0]`; second call blocks |
| Historical base fixture | RED: same pre-existing defect |
| Corrected fixture | GREEN: `[0,1]`; both callers return exact JSON, one writer |

Red logs retain full shell stderr, PID/status, `/proc/PID/wchan` and syscall state.
The second caller is sleeping in `wait_for_partner`, syscall 257 (`openat`), after
its trace reaches `printf x`. The sole reader has exited, the first caller has
returned, and the second has no JSON output. This is direct FIFO-open evidence,
not a diagnosis based only on a timeout. The two-second observation bounds cleanup;
it is not used to create the interleaving. All owned process groups are killed if
needed and reaped; successful callers and reader exit zero. Temporary FIFOs and
markers are removed. There is no daemon/store in this lowest-layer probe.

Competing hypotheses considered:

- **Duplicate rendezvous owner:** directly reproduced on candidate and base,
  eliminated by changing only the ownership operation.
- **Missing readiness or unsuccessful executable replacement:** inconsistent with
  the historical failure location after both operations; this is not their timeout.
- **Lost runtime fence event / another daemon defect:** not ruled out historically.
  T-764 concerns snapshot-local runtime latching/event persistence, a different
  defect. Its existing snapshot regression and production fixes are left intact.

## Targeted checks and cost

- `/bin/sh -n crates/corral/tests/version_skew/herdr.sh`: pass. Shell LSP is
  unsupported in this session; the syntax check supplies changed-file validation.
- `python3 .evidence/T-954/fifo_regression.py FIXTURE`: candidate red, base red,
  corrected green, each once; full logs and the probe are attached. No flake loops,
  stress tests, intentional load, or load-based inference.
- `bash scripts/cargo.sh test -p corral --test version_skew_cli --features
  test-skew-barrier -- --exact daemon_exits_for_upgrade_after_its_executable_is_replaced
  --nocapture`: pass, 2.14s execution.
- Same targeted command with
  `daemon_persists_snapshot_skew_before_exiting_for_upgrade`: pass, 0.27s execution.
  Both use a private temporary store, copied actual binary, fake terminal, real
  inode replacement and unchanged 120s hang guards. Exit 75, exactly one skew event,
  and absence of dispatch/review/gate events remain exact assertions.

No added test registrations or permanent suite seconds: the deterministic probe
and raw output are retained as evidence, rather than adding another daemon E2E or
a recurring race-test lane. Existing tests are not relaxed, quarantined or retried.
The existing quarantine manifest has no entry for these tests.

Scope is the fixture plus this report. No T-935 worktree edits, production changes,
live daemon operation, installation, owner-pane access, PID allocator changes, or
intentional host load occurred. `cxm context` and the required initial broad Corral
search timed out without output; exact Git snapshots, direct inspection and bounded
read-only research were used instead. Gate results are recorded separately by the
run's receipted quick gate after final commit/rebase.
