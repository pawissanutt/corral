## PM configuration reference

Consult this reference whenever the owner requests configuration, automation, or an
“always/never” instruction. Translate the request to the narrowest durable setting;
inspect current effective values and authority first. Apply authorized changes yourself,
not by asking the owner to edit files. Do not invent a grant or treat a preference as
consent. If the instruction has no supported durable representation, say so and ask for
an appropriate decision rather than claiming it is enforced.

At startup and after policy changes read `corral mode status`, `corral automation status`,
`corral config get review --source`, and `corral config get dispatch --source`; check
`corral pm brief --json` for effective policy and blockers. Policy delivery supersedes
older instructions. Config is durable, but finite grants expire and must be checked again.

Automatic transitions require the sync loop: corrald owns it when running; otherwise
the board and hooks retain their legacy passes. Use `corral daemon status` to read
health; owner service setup is `corral daemon install`. `pm.supervise` opts into PM
recovery by corrald, not permission to dispatch or resolve owner decisions.

| Owner intent | Action and readback |
|---|---|
| Automate dispatch, review or landing | Use `corral mode autopilot --task T-042 --dispatch` to preview; include `--accept` only if landing is requested. Inspect scope, exclusions, expiration and digest. Only the owner can confirm the exact preview interactively with `corral automation confirm --task T-042 --dispatch --digest <preview-id>` (include the same selection and authority flags). Never edit generated grant identity fields. For project/future tasks use the supported bounded `--include-new` preview options, not an unlimited promise. Check `corral automation status`. Assist is the default; review settings alone confer no authority. |
| Pause or resume automatic actions | `corral mode pause` or `corral mode resume`; inspect `corral mode status`. Pause does not stop running work; resume does not enlarge or renew a grant. |
| Limit work or admit a project | Set `limits.wip`, `limits.running`, or `limits.running_target` via `corral config set <key> <value>`. Use `corral queue admit <project>` or `corral queue admit <project> --off`; inspect `corral queue list --json` and `corral config get dispatch.ready_projects --source`. Empty admission list means all projects, not none; use `corral queue pause` to stop all admission. Admission changes can void prepared dispatches: inspect queue and prepare again rather than assuming a reservation survives. |
| Change gates, tiers or review | Configure `review.gate`, `review.gate_tiers`, `review.tier_policy.plan_member`, `review.tier_policy.review_rounds`, `review.reviewer` or `review.accept` as appropriate, including project overrides. Consult the configuration guide for valid values and tier definitions. Explicit owner acceptance stays owner-only; auto acceptance still needs a matching grant. Inspect `corral show T-042 --json` and the task's gate contract. |
| Change model routing or profiles | Use `routing.rules`, `routing.reviewer_rules`, `projects.<project>.default_profile`, `defaults.profile`, and `profiles.<name>` as appropriate. For profile setup use `corral setup status` and `corral setup catalog` and the model-selection procedure. Preview with `corral route T-042 --json` and refresh discovery with `corral harness list --refresh`. Task pins win over routing; reviewer precedence is task override, project reviewer, first matching reviewer rule, global reviewer, then none. A project reviewer therefore masks reviewer rules. |

A minimal workspace policy fragment (illustrative, not an automation grant):

```toml
[limits]
wip = 2
running = 3
running_target = 2

[dispatch]
ready_projects = ["app"]

[defaults]
profile = "example"

[projects.app]
path = "app"
default_profile = "example"

[profiles.example]
harness = "claude"
model = "example"

[[routing.rules]]
profile = "example"

[[routing.reviewer_rules]]
profile = "example"

[review]
reviewer = "none"
accept = "owner"
gate = []

[review.gate_tiers]
quick = ["true"]

[review.tier_policy]
review_rounds = "full"
plan_member = "full"
```

For example, preview landing authority with `corral mode autopilot --task T-042 --dispatch --accept`;
for future work, preview with `corral automation preview --project app --include-new --dispatch`.

Use `corral config get <key> --source` before and after `corral config set <key> <value>`;
use `corral config validate` and `corral config history` / `corral config diff` to verify
the recorded change. Workspace settings override user preferences. Save old values and
sources before changing them; roll back with a validated set of the previous value at
the same layer, then read back and validate. Removing an override to restore inheritance
may require a manual file edit; validate it and inspect history. Consult
`docs/guide/configuration.md` and `docs/guide/automation.md` for schema and exact flags.
Config changes can stale in-flight gates or reviews, and changing effective reviewer rules
invalidates affected attempts. Inspect active work and receipts before changing policy;
never treat old results as current. Queue removal or pause does not cancel running agents.
Grant confirmation, paid spend, privacy, and other owner-only authority require the
owner's decision; routine supported configuration changes do not require repeated consent.
