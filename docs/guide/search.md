# Searching tasks

```bash
corral search "retry project:app -status:dropped"
corral search "has:artifact cost>5" --json
corral list --query "profile:reviewer since:7d" --json
corral list --query "retry" --status done --project app
corral list --done --query "sha:abc123" --since 7d
```

`search` and `list --query` search **all statuses**, including Idea, Done and Dropped.
Results are ranked by relevance, with exact and substring matches above fuzzy word
subsequences. Task ids and titles carry more weight than run summaries, artifact
titles and task bodies. Search also includes project, kind, status, profile, run
model and harness, and landed commit metadata. Artifact contents are not searched.
Equal scores use newest task update first, then highest id.

`list --status` and `--project` intersect with the query, rather than replacing it.
Without `--query`, `list` keeps its normal id order and hides Done and Dropped unless
`--all` or an explicit status selects them. `list --query` supports `--columns`.
Both search commands accept `--json`, returning the existing list task array in
result order; no matches produces `[]`. Invalid queries fail with an error on stderr
and a nonzero exit status, including a named error for unknown fields.

`list --done` still selects only Done tasks, newest finished first, with finish and
landed metadata. Adding `--query` filters that list without changing its order.
Its `--since 7d` option measures **finish time**; `since:7d` inside the query measures
**task update time**.

## Shared query syntax

The board finder, history filter, `search` and `list --query` use the same parser.
Every whitespace-separated term must match (AND). Text matching is case-insensitive.
Prefix a term or field with `-` to exclude it, for example `-obsolete` or
`-status:dropped`. Quote the whole query for the shell so spaces and `>` reach corral.
For a query beginning with `-`, use `corral search -- "-status:dropped"`, or
`corral list --query="-status:dropped"`.

| Syntax | Meaning |
|---|---|
| `retry` | Text match, including fuzzy subsequences within a word |
| `"retry failed"` | One text term containing spaces |
| `status:done` | Exact lifecycle status, using the usual status names and aliases |
| `project:app`, `kind:fix` | Case-insensitive substring in that task field |
| `profile:reviewer` | Task preference or any saved run's profile (legacy agent key included) |
| `model:opus`, `harness:omo` | Substring in saved run metadata |
| `id:T-04*` | Task id pattern; `*` matches any number of characters |
| `pinned` | Task is pinned, or one of its artifacts is pinned |
| `since:7d` | Updated at or after the cutoff |
| `before:2026-01-01` | Updated strictly before the cutoff |
| `cost>5` | Known task cost strictly above the USD threshold: reported spend plus estimate-only usage, including reviews |
| `rounds>2`, `tokens>1000` | Task rounds or total recorded tokens strictly above the threshold |
| `has:artifact` | Task has at least one artifact record |
| `sha:abc123` | Substring in landed commit metadata; retained for history compatibility |

Dates accept relative hours, days or weeks (`12h`, `7d`, `2w`), a UTC calendar date
(`YYYY-MM-DD`), or an RFC3339 timestamp. Relative dates are resolved when the query is
parsed. Numeric thresholds must be nonnegative; malformed numbers, dates, empty
field values and unclosed quotes are errors.

Quote field values containing spaces inside the query, for example
`corral search 'project:"my app" retry'`. A fully quoted token is literal text,
so `corral search '"https://example"'` searches text containing a colon instead of
treating it as a field.

History remains limited to Done and Dropped tasks in newest-finished order even
though it accepts the shared syntax. Its old `project:`, `kind:`, landed SHA and
plain text searches remain supported. Use the board finder or CLI search for
active and archived work together.
