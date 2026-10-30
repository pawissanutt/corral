# PM cross-lane fixture contract

`cross-lane.test.ts` runs only in the private plugin-free environment supplied by
`scripts/test-herdr.sh`. It builds the checkout's Rust CLI and corral-ops test binary
through the existing package support helpers. It never starts or reuses an owner server.

The parent Rust implementation must supply this ignored test:

```text
agent::hooks::pm::tests::pm_cross_lane_setup
```

Input is `CORRAL_PM_LANE` JSON:

```json
{"root":"/private/temporary/workspace","workspace":"fixture-workspace-id","pane":"fixture-pane-id"}
```

Initialize a store under `root`, two eligible tasks in distinct projects, and one
AgentPm bound to the first project and the supplied pane. Save the private fixture
client's socket explicitly. Prepare a pending hello with host capability `"e".repeat(64)`
and bind nonce of your choosing. The TS host supplies ctl capability `"d".repeat(64)`.
Do not pre-bind a Pi session: the real host calls hello with its session file and PID.

Print one newline-terminated output line:

```text
PM_LANE {"store":"st-…","project":"inside","task":"T-001","outside_task":"T-002","generation":1,"nonce":"…","sessions":"/private/temporary/workspace/sessions"}
```

`sessions` must satisfy Rust's managed PM session-path validation. Put any runtime state
inside the private environment so the enclosing helper owns cleanup.

The fixture sends model-issued comments to both targets, verifies exactly one durable
comment and PM receipt on the scoped task, and observes `out_of_scope` for the other.
The host uses production PM, auth and delivery modules with synthetic credentials and
an offline transport. No paid calls, sleeps, sync or board action executor are involved.

Killed-executor capture and interrupted legacy-reply recovery remain Rust fixture
responsibilities; this setup contract does not claim that those cases are covered.
