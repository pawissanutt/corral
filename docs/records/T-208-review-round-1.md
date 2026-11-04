# T-208 — changes round 1, F1

Addressed the review's sole blocking finding: injected CLI acceptance could override an
explicit user `crossSessionInbound: hold` or `refuse`. PM clarification
T-208/R1/R1-review1/Q1 revision 2 requires those user restrictions to block as well.

The shared policy resolver now combines CLI and user values using restrictive `max`,
matching the managed/project/local merges. When neither source supplies a value it still
defaults to Hold. The same resolver is used by readiness and pre-send revalidation; no
caller-specific workaround or fallback was added. Policy::Hold maps to InboundHold and
Policy::Refuse to PolicyRefused in the existing readiness mapping.

One pure policy-level regression covers user hold/refuse with CLI accept, absent-source
blocked default, and acceptance without a user restriction. It uses two tiny JSON
objects, not another socket journey or live fixture.

Failing first: `cargo test -p corral-ops --lib
 delivery::adapters::claude::policy::tests::cli_accept_cannot_override_explicit_user_hold_or_refuse`
failed on the old merge with actual Accept versus expected Hold (0.00s execution).
After the fix: `cargo test -p corral-ops --lib delivery::adapters::claude` passed all
4 tests in 0.12s, including the existing fake-socket/transcript journey. Red/green logs
are attached separately. This adds one small policy test; no overlapping
end-to-end test or load test was added.

The original capture and authorized narrow version-pinned peer-user isMeta exception
remain unchanged. Supported range and fail-closed remote/dynamic-policy limitations
remain as documented in T-208-implementation.md. The final rebased quick gate's fresh
receipt is attached by corral, not replaced by the previous round's receipt.
