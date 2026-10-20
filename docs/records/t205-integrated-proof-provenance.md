# T-205 integrated proof provenance

The October 20, 2026 R3 review identified a test-bearing integration after the
worker's initial submission. The original no-integration-delta statements in
`T-205:R3:536a955f-proof-audit.md` and
`T-205:R3:8280f9ee-completion-report.md` describe the earlier submission only.
They do not establish repeated-suite coverage for the integrated candidate.

The original five-suite records identify HEAD
`46a63bcd5782961b404e14eac5c744205b6e3a46`, Git tree
`f048e7cb133a5cb039f5682aeff673357108b6d1`, and integration base
`d20996a7050d42d0a81a1f89b1df1d9be9393e75`. Each records 2,536 passing tests
across 121 binaries.

The reviewed integrated candidate is HEAD
`c3e6c17926fd21832860a37cc5b0c91919279bb6`, Git tree
`d4e758bcca75f621a36a8e415c2b5249433ce428`, and integration base
`206178859091d52cb4ac4da690223f295dc0f646`. Main commits `7812e60`,
`6beb0a7`, and `2061788` add or change tests, including Git target tests,
analytics tests, and the shared-server Plan-target test. Its configured gate
executes 2,591 tests across 123 binaries.

The old records remain valid observations of their original source, not proof
for the additional tests. A single passing integrated gate does not satisfy
the five-consecutive-suite requirement. The review's sole requested change is
renewed proof and corrected provenance; it identifies no code defect.

Renewal uses the existing committed driver, from a clean candidate after
integration and this provenance correction are committed:

```sh
python3 scripts/qa/t205-nextest-proof.py \
  .omo/evidence/T-205-R3/review-round-1-proof </dev/null
```

The driver executes five consecutive unfiltered workspace nextest suites. Each
owns a fresh private plugin-free herdr server, uses null stdin, and records
before/after HEAD, Git tree, integration ancestor, observed main tips, original
exit status, summary, and wall time. It stops on the first failure or candidate
identity change. The configured 12-slot cap, zero retries, and serial herdr
group also apply to the newly integrated `_live` binary.

The renewal's raw logs and identity records, and the subsequent configured
gate log, belong in the T-205 artifact registry. The current review-response
report must name their exact tested identity and actual test count. Earlier
reports and the matched-source historical timing/resource benchmark retain
their original identities and must not be relabeled onto this candidate.
