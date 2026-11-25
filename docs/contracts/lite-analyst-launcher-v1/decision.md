# T-2076 / R1 · owner disposition

**Authorized to implement the real Corral-side E2E path within the fixture boundary only.** The owner selected “Authorize implementation within exactly that scope (recommended)” in `q-2b5efe91a74d17819c87fd020f81b3b3`. The question specified same-host Linux, default off and explicitly opted in per disposable run, only pinned public synthetic fixtures, a scripted local provider and no internet egress.

This is the implementation decision for the Corral C2–C4 chain (T-2077 / T-2078 / T-2079), not a decision to enable it, change any production default or promote a passing E2E to rollout. It extends the `T-2069:4` spike-only endorsement in exactly this scope. No private vault, live provider, remote transport, rollout, owner RPC reuse, coding handoff, or change to external MCP defaults. Lite's own grant, retention, and computed capture remain separate decisions in Lite. This contract does not grant one run authority: the two hosts must still require explicit fixture opt-in, enrollment and per-run consent, and fail closed on any unmet bound.

**Recommendation, not additional authority:** implement against the pinned local launcher profile, then have the joint runner exercise real product hosts and preserve independent replay evidence. Leave disabled, and report unmet platform, isolation and compute gates rather than using a permissive substitute.
