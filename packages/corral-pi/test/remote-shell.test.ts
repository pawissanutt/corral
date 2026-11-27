import { expect, test } from "bun:test";
import { registerShell } from "../src/host/remote/shell.ts";
import { capabilities } from "../src/host/remote/state.ts";
import { codeHost } from "./support/code-host.ts";

test("remote shell uses Pi's bash path, records context policy, and refuses a busy turn", async () => {
  const fx = await codeHost("assistant");
  const { host } = fx;
  const completed = Promise.withResolvers<string>();
  registerShell(host.remoteRegistry, () => host.session, run => host.runRemoteShell(run), completed.resolve, 200);
  const shell = host.remoteRegistry.get("owner_bash")!;
  try {
    expect(capabilities(host.remoteRegistry, { open: false, closing: false, rewind: false }).some(can => can.id === "shell")).toBeTrue();
    expect(await shell({ command: "printf '%s' \"$PWD\"", exclude_from_context: false })).toEqual({ text: `exit 0\n${fx.cwd}` });
    expect(await shell({ command: "printf hidden", exclude_from_context: true })).toEqual({ text: "exit 0\nhidden" });
    const entries = host.session.sessionManager.getBranch().filter(entry => entry.type === "message" && entry.message.role === "bashExecution");
    expect(entries.map(entry => entry.type === "message" && entry.message.role === "bashExecution" && entry.message.excludeFromContext)).toEqual([false, true]);
    expect(await shell({ command: "sleep 0.4; printf later", exclude_from_context: false })).toEqual({ text: "Shell command running; its result will appear in the session." });
    await expect(shell({ command: "printf no", exclude_from_context: false })).rejects.toMatchObject({ code: "busy" });
    expect(await completed.promise).toBe("exit 0\nlater");
    const turn = fx.turn([{ toolCalls: [{ callId: "hold", name: "fixture_hold", arguments: { name: "shell" } }] }, { text: "done" }]);
    try {
      await fx.holds.get("shell").started;
      await expect(shell({ command: "printf no", exclude_from_context: false })).rejects.toMatchObject({ code: "busy" });
    } finally { fx.holds.get("shell").release("done"); await turn; }
  } finally { await fx.close(); }
});
