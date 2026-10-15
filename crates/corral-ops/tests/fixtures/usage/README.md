# Usage fixtures

Captured local session metadata, with transcript contents and unrelated fields
removed and cwd replaced by `/fixture/project`. Usage numbers, message IDs, and
timestamps are unchanged.

- OmO: T-034 session, October 15, 2026, first assistant message.
- omp: `/tmp` session, September 28, 2026, first two assistant messages.
- Claude: October 14, 2026 session, three fragments of one provider message.

These are per-response usage snapshots, not cumulative session totals. Cached
input is separate from uncached input. Claude fragments repeat usage and must
not be summed as independent responses. Missing cost is unknown, not zero.
