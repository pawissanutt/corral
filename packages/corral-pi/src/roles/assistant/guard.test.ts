import { expect, test } from "bun:test";

import { assistantShellRefusal } from "./guard.ts";

test("pane input is refused however the herdr command is reached", () => {
  for (const command of [
    "herdr pane send-text w1:p1 'hello'",
    "herdr pane send-keys w1:p1 Enter",
    "herdr --session corral-test pane run w1:p2 'corral approve T-5'",
    "/usr/local/bin/herdr agent prompt w1:p1 'do it'",
    "echo ok && herdr pane run w1:p1 ls",
    "echo $(herdr pane send-text w1:p1 x)",
    "bash -lc 'herdr pane send-keys w1:p1 Enter'",
    "env HERDR_SESSION=x herdr pane run w1:p1 ls",
  ]) {
    expect([command, assistantShellRefusal(command)?.startsWith("assistant rule: no input into a herdr pane")])
      .toEqual([command, true]);
  }
});

test("direct writes under .corral/ are refused; reads and corral commands are not", () => {
  for (const command of [
    "echo note > .corral/tasks/T-9.md",
    "printf x >> /ws/.corral/state/assistant-counter",
    "echo x >| .corral/a",
    "cat draft.md 2>/dev/null > \".corral/tasks/T-9.md\"",
    "rm -rf .corral/state",
    "sudo rm .corral/corral.toml",
    "cp /tmp/x.md .corral/tasks/",
    "mv draft.md /ws/.corral/tasks/T-9.md",
    "mkdir -p .corral/inbox/pm",
    "echo x | tee -a .corral/events.jsonl",
    "sed -i 's/a/b/' .corral/corral.toml",
    "dd if=/dev/zero of=.corral/blob bs=1 count=1",
    "sh -c 'touch .corral/x'",
  ]) {
    expect([command, assistantShellRefusal(command)?.startsWith("assistant rule: no direct write under .corral/")])
      .toEqual([command, true]);
  }
  for (const command of [
    "cat .corral/tasks/T-1.md",
    "ls -la .corral",
    "grep -rn \">\" .corral/tasks",
    "sed -n 1,20p .corral/corral.toml",
    "corral comment T-1 'looks good'",
    "corral new 'Fix X' -p corral --idea --body-file /tmp/spec.md",
    "corral show T-1 --json > /tmp/t1.json",
    "herdr pane list",
    "herdr pane read w1:p1 --lines 20",
    "echo ok > notes.txt",
    "git status",
  ]) {
    expect([command, assistantShellRefusal(command)]).toEqual([command, undefined]);
  }
});
