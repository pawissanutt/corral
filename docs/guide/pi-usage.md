# Pi usage inspection

Open **Usage** from F2 (Views) or `/panels usage` (see [native panels](pi-panels.md)). It is read-only, not a tab (its place reads `Chat › Usage`) and stays bound to the launched session, and to its task and run when it has them, even when the Files repository changes. F9 returns to the same conversation; the editor is not replaced. Scroll with arrows, paging or wheel; `?` shows the reading rules. There is no function key for Usage and no click on the status bar's cost or context.

The page has one live section, **Session**. Views shorter than 12 body rows open with a one-row digest; the full section follows it.

## Session

Tokens, the API-equivalent USD estimate, the context window (`48,000 / 200,000 · 24%`) and the model with its effort. An unknown value reads `unknown` (a context share without a known percentage reads `percent unknown`). A build session with agents lists this pane and each of its build agents as rows with a `total` row; the total cost is the status bar's rule (the parent plus the agents' known costs), and a `+` beside a total marks a member whose value is unknown. When the table does not fit, each row keeps its token and cost figures; context remains available in Agents and the status bar. Helper calls (Explore) are neither listed nor added. Session tokens and cost are counted by this pane and priced from the model catalog: an estimate, not a bill or a provider quota.
