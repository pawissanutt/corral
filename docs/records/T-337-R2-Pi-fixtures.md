# T-337 R2: canonical Pi fixture port (Q8)

PM answered replacement Q8 and amended the task write scope to test helpers and `src/**/*.test.ts`. No production host TypeScript changed.

The first full Pi result was **480 pass / 19 fail**: integration setup and assertions still treated v1 event/inbox/state/artifact files as authorities. Later assertions in those files had not executed. Restoring mirrors would violate storage-v2.

## Narrow port

- A genuinely test-only, ignored Rust Ops probe uses Store/EventReader, Inbox, gate-path and artifact-reader APIs. It adds no public CLI, production authority or SQL access.
- The canonical event follower subscribes to filesystem writer-close notifications on the generic Store root **before** capturing the sequence tail. It acknowledges that captured cursor before the test triggers its action. Notifications are only hints: all events and cursors come from the canonical bounded EventReader. Real fixture writers are corral command processes; writer close follows their commit. The observer blocks on notifications, not sleeps or polling. Notifications coalesce in a bounded one-slot channel; an error is retained separately and fails the observer rather than being discarded.
- The reader advances monotonically by event sequence; it cannot replay pre-subscription history or duplicate queued notifications. A separate process remains able to observe commits while the parent JS process waits synchronously. The new integration regression checks historical-tail exclusion, rapid real commits (including identical comment bodies), complete canonical event equality and absence of an events.jsonl mirror.
- The existing raw-file follower and all seven partial-read/truncation/replacement/notification regressions remain intact. They are no longer the Store authority for cross-lane integration.
- Inbox assertions read real canonical messages, receipts, envelopes and verified frozen payloads. Existing kind/count/body, receipt/session/generation, payload ordering and digest assertions are retained. The old nudge YAML substring assertion is expressed as the same exact kind and structured subject generation.
- Shell launch-exit notifications and gate control files remain nonauthoritative scratch protocols. Fixtures obtain their paths through the existing Store/gate resolvers, rather than assuming `.corral/state` directories. Their existing notification, exit-code and barrier assertions are unchanged.
- Artifact HTML/log assertions use the existing verified artifact reader, not a presumed snapshot file. C11 compares the complete canonical task record before/after, rather than a nonexistent task Markdown mirror; owner-input journal checks use assistant show's canonical container.

No integration was replaced with a mock, no failed case was skipped, no timeout or performance counter was relaxed, and no production behavior or storage layout was changed.

## Results

The initial canonical follower run preserved all seven raw-file tests and passed the new canonical notification test. It then exposed a further artifact-path assumption in the export assertion; that was ported to the canonical artifact reader, without removing its content/deduplication assertions.

Full private plugin-free Pi suite after the port: **507 pass, 0 fail, 5,619 assertions, 113 files**, terminal job j1-7 exit0 in148.98 seconds (`R2-Q8-pi-full-first-port.log`, artifactdac4a851). The increased count reflects integration tests previously blocked by failing beforeAll hooks, plus the new canonical follower regression. Typecheck also passes.

## Final main delta

The next actual freshness assessment found one relevant main landing beyond d9749933: **2f5e7f92794496010874b44d0f5e0191d20b3fdc**, “Return approved runs to review gates after changes.” It affects the real gate contract, not just a moved SHA, so merge555686ee carries it without resetting any retained work. Approved runs return to Review/quick after a newer/equal changes marker; only a renewed later approval selects Landing. Changes rounds are recorded for Fresh worker sessions too, while the conversation notice remains conditional. The port retains RunHead access and the Store clock seam.

The first affected subset reached98 pass / 3 fail, with20 not run due fail-fast. All three failures were the new main regression expecting a full identity inside `gate_started`; v2 deliberately stores slim event references. The test now checks the exact attempt reference and the **complete original CandidateIdentity from its canonical GateAttempt identity blob**, retaining the historical-evidence assertion rather than restoring full event payloads or dropping it. Current-cut revalidation then completed: **121/121 affected Ops tests pass**, including every incoming changes-round scenario, pure phase cases, full/quick freshness, and exact nonce model; the three unchanged board/read/sync budgets each pass (10.27s,61.43s,62.70s); Pi typecheck passes and the complete Pi suite again passes **507/507**,0 failures, in169.11s. Terminal j1-9 exit0, `.evidence/R2-2f5e-final-affected-and-Pi.log`. This includes the bounded one-slot notification refinement. The final 800-SLOC check passes across1668 source files. Main still equals2f5e7f92 and storage-v2 remains83dcc33e; there is no unintegrated main delta. The single contracted quick gate remains the final four-package Rust regression and current-cut action-budget check, not replaced by this affected subset.
