# Pi usage inspection

Open **Usage** in the main navigation or `/panels usage`. This is read-only and stays
bound to the launched task/run even when the Files repository changes. F9 returns to
the same conversation; the editor is not replaced. Scroll with arrows, paging or wheel.
`r` rereads canonical stored accounting; it does not run a collector or contact a provider.

The source table comes from existing stored collector snapshots. Task totals cover
worker runs (including missing-run coverage); selected-run model rows derive from that
worker's response entries. Reviews are explicitly excluded. Active responses can lag;
missing usage, legacy model/timing evidence, failed reads and retained stale data are
not zero. Completed wall time is not model-active time; response latency is the
collector's transcript-derived latency, not throughput. Up to 100 model rows are shown,
with any omitted count retained. Source strings retain pricing provenance.

Reported USD is transcript-reported cost, not independently verified billed metered
spend. API-equivalent estimates are separate: subscription usage does not imply an
extra subscription charge. Unpriced token counts and partial worker coverage remain
visible. The UI neither reprices history nor decorates or recommends models.

`c` toggles token-category bars from each source row. Exact numeric source tables are
always retained, including on compact terminals; bars are relative to the largest
category in that row, not a provider quota or a percentage of a budget.

The budget section uses the bound host's last canonical accounting receipt. Configured
token/USD limits are distinct from settled-plus-reserved totals. Unresolved/interrupted
requests retain reservations; pricing gaps, legacy usage gaps and unresolved unbounded
requests remain explicit. Missing receipt fields mean unavailable, not unlimited.
These accounting USD figures are not billed spend, remaining subscription balance or
provider quota. Cancellation is best-effort and provider billing may lag.

Unbound/assistant sessions without a task/run inspection show collected facets as
unavailable. No cross-session attribution is invented.
