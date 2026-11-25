# Lite analyst v1 fixture handoff

**Offline synthetic baseline, not implementation or private-data approval.** The [contract](contract.md) defines the boundary; the [Corral owner decision](decision.md) is recorded separately and is not an input to policy or bundle digests.

Bundle ID: **`sha256:9a0376354d43efe1b72ffef0be9ee333cedf95cd50890a096f9c891c0b4607a5`**. This is the SHA-256 of the exact [`SHA256SUMS`](SHA256SUMS) bytes (four entries in fixed order with LF endings, including the final LF). It is distinct from all wire and policy digests. From this directory:

```sh
sha256sum SHA256SUMS
sha256sum -c SHA256SUMS
```

| Checked file | Purpose |
|---|---|
| `contract.md` | Normative proposed control DTOs, fences, digest preimages and authority boundaries |
| `synthetic-prompt.txt` | Exact synthetic rendered system-prompt bytes, including terminal LF |
| `tool-policy.json` | Provider descriptors and host-only tool list, with closed JSON Schema argument and reply shapes (Draft 2020-12 vocabulary) |
| `vectors.json` | Synthetic model/request/permit/report bytes and hashes, evidence responses, and expected failure/race outcomes |

`vectors.json` is fixture data, not a new protocol envelope. Each signed vector has `body`, `canonical_utf8` and `sha256`; construct wire fields as the contract specifies. `grant` is only a synthetic commitment preimage, not a Lite storage schema. `model_request_b64` is one example provider request for step 1, not a provider protocol or a full trace. Evidence invocations represent a later sequence of model and host activities; no paid turn corresponds to `read_result`. Cases in `scenarios` are independent from one another; resets, overrides and fault injection belong in the harness. Use deterministic barriers for the race cases. The February fixture supports 5,100 − 2,800 = **2,300**, not a cash balance.

**Coordinate on this bundle.** T-2070 uses the exact provider-visible descriptors, MCP schemas, and closed-registry/canary cases. T-2071 uses the approval, turn, ledger, report and failure cases against a fake Lite peer and scripted provider. A Lite peer may copy the verified bundle and test its own boundary separately. Record the bundle ID and provenance (T-2069 and its artifact or commit) in receipts; compare the ID before interoperating and review any proposed changes in both consumers before adopting a new digest. No sibling checkout, shared source dependency or live vault is needed. This task validates the published data and digests, not the behaviors described by the scenarios.
