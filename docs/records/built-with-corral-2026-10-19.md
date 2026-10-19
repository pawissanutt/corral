# Built with corral: October 19, 2026 snapshot

This is a dated provenance record for the README, captured from the repository and the owner's
development task store on October 19, 2026. It is not a benchmark, a claim of autonomous development,
or an assertion that every task received the same checks.

## Public repository and release

- First commit: [`75c8cfa`](https://github.com/pawissanutt/corral/commit/75c8cfa),
  October 14, 2026, "corral: prototype of the herdr-based project management TUI".
- Tag: [v0.3.0](https://github.com/pawissanutt/corral/releases/tag/v0.3.0),
  October 19, 2026; points to release commit `4fcdbea`. A tag is not proof of published packages;
  the release workflow publishes only after validation and native packaging pass.
- Counted main snapshot:
  [`630811eb1b8ca328bc9d631e4516e0e024e58edc`](https://github.com/pawissanutt/corral/commit/630811eb1b8ca328bc9d631e4516e0e024e58edc).
  It contains 369 commits, of which 343 have `(T-` in the subject.

From a full clone (not a shallow history):

```sh
git log 630811e --reverse --format='%h %ad %s' --date=short
git rev-list --count 630811e
git log 630811e --format=%s | grep -c '(T-'
git show -s --format='%h %ad %s' --date=short 'v0.3.0^{commit}'
```

Once the release workflow publishes its assets, inspect publication separately with
`gh release view v0.3.0 --json tagName,publishedAt,url,assets`.

The count is one per commit subject containing the literal `(T-`, including subjects that mention
multiple tasks or add text before a closing parenthesis. It does not count unique tasks or prove
review. The narrower pattern `grep -Ec '\(T-[0-9]+\)'` returns 335 at this same revision.
The task's original 335/361 count predates this snapshot; its denominator must not be mixed with
the refreshed history.

## Development task store

The owner's workspace returned **149** records with project `corral` and status `done`.
Reproduce against that workspace (requires `jq`):

```sh
corral list --json --all |
  jq '[.[] | select(.project == "corral" and .status == "done")] | length'
```

<details>
<summary>Captured Done task IDs (149 records)</summary>

```text
T-001 T-002 T-003 T-004 T-005 T-006 T-008 T-009 T-010 T-011
T-012 T-013 T-014 T-015 T-016 T-017 T-018 T-019 T-021 T-022
T-023 T-024 T-025 T-026 T-027 T-028 T-029 T-030 T-031 T-032
T-033 T-034 T-035 T-036 T-037 T-038 T-039 T-040 T-041 T-043
T-044 T-045 T-046 T-047 T-049 T-050 T-051 T-052 T-053 T-054
T-055 T-056 T-057 T-058 T-059 T-060 T-061 T-062 T-063 T-064
T-065 T-066 T-067 T-068 T-069 T-070 T-071 T-072 T-073 T-074
T-075 T-076 T-077 T-078 T-079 T-080 T-081 T-082 T-083 T-084
T-085 T-086 T-087 T-088 T-089 T-090 T-091 T-092 T-093 T-094
T-095 T-096 T-097 T-098 T-099 T-100 T-101 T-102 T-104 T-105
T-106 T-108 T-109 T-110 T-111 T-112 T-113 T-114 T-131 T-132
T-133 T-134 T-135 T-136 T-142 T-143 T-144 T-145 T-150 T-151
T-152 T-160 T-161 T-162 T-163 T-164 T-165 T-166 T-167 T-168
T-169 T-170 T-171 T-172 T-174 T-175 T-181 T-182 T-183 T-184
T-185 T-186 T-188 T-189 T-196 T-202 T-204 T-212 T-216
```

</details>

The `.corral/` development store is local workspace data, not shipped in a fresh clone. This is a
reported store snapshot, not a count that a fresh install will reproduce. Counts will change as
development continues. Done records alone do not prove a passing gate, independent review or
merged code; those require the run receipts and landing events.

Two inspected examples show the complete workflow:

| Task | Work | Gate | Independent review | Recorded local landing |
|---|---|---|---|---|
| T-101, R1 | Earlier PM-first README | pass on `e4544899` | `gpt-sol`, approve on `e4544899` | October 16, 2026, 06:14:09 UTC; rebase |
| T-108, R1 | Native release packaging and installers | pass on `70a69149` | `gpt-sol`, approve on `70a69149` | October 17, 2026, 23:23:49 UTC; fast-forward |

Public landed commits: [T-101](https://github.com/pawissanutt/corral/commit/e4544899ac9df32a113752e0276e8a881c7e4667)
and [T-108](https://github.com/pawissanutt/corral/commit/70a691496ac48558a8d7fb154940354d79546baf).
The gate and reviewer results above are transcribed from the local run records; the landing times
come from their `approved` events. Independent review here means a separate reviewer session,
not necessarily a different model vendor. Inspect those receipts in the development workspace:

```sh
corral show T-101 --json
corral log T-101 --json
corral show T-108 --json
corral log T-108 --json
```

T-108's landing event also records a failed worktree cleanup; the code landed, but cleanup did not
complete in that event. Its October 17 review approved release preparation, not publication.
The current release tag is verified separately above; the historical landing records do not prove
that its packages have been published.
