### Test-only and flaky-test review (v1)

Everything in the implementation contract applies, plus:

- The fix is deterministic: no added sleeps, polling delays, loosened time budgets, retries
  that mask failure, or weakened or deleted assertions.
- The regression test can fail: identify the exact assertion that fails without the fix and
  confirm it exercises the original defect, not a proxy.
- Asynchronous behavior subscribes to the exact event or state change before triggering it
  and awaits it with a bounded timeout.
