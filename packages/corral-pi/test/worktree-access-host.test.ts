import { expect, test } from "bun:test";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../src/bridge/events.ts";
import { APPROVAL_CHOICES as CHOICE } from "../src/host/pending-questions.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { WORKTREE_ACCESS_ENTRY } from "../src/host/worktree-access.ts";
import { tryWorktreeLock } from "../src/host/worktree-files.ts";
import { taskDestinationLock, taskWriterLock } from "../src/host/worktree-writer.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { CorralClient } from "../src/tools/corral.ts";
import fixtureAuth from "./fixtures/auth/index.ts";
import { fakeCodexTransport, type FakeStep } from "./fixtures/auth/fake-transport.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const task = "T-42";
const call = (callId: string, name: string, args: Record<string, unknown>): FakeStep => ({
  toolCalls: [{ callId, name, arguments: args }],
});

test("assistant checkout takeover uses the owner card and guards direct and nested SDK writes until hand-back", async () => {
  const root = mkdtempSync(join(realpathSync(tmpdir()), "corral-takeover-host-"));
  const fetch = globalThis.fetch, env = { ...process.env };
  let host: HostSession | undefined;
  try {
    const cwd = join(root, "checkout"), checkout = join(cwd, ".worktrees", task), state = join(root, ".corral-next");
    const source = join(checkout, "source.txt"), lock = join(state, "locks", `${task}.checkout`);
    const protectedPath = join(checkout, ".git"), outside = join(cwd, "outside.txt");
    const link = join(checkout, "escaped.txt"), sibling = join(cwd, ".worktrees", "T-43", "source.txt");
    mkdirSync(checkout, { recursive: true });
    mkdirSync(join(state, "locks"), { recursive: true });
    writeFileSync(join(state, "corral.toml"), "");
    writeFileSync(lock, "");
    writeFileSync(protectedPath, "metadata");
    writeFileSync(outside, "outside");
    symlinkSync(outside, link);
    const inode = (path: string) => lstatSync(path, { bigint: true });
    const result = { task, run: "R1", checkout, lock, device: String(inode(checkout).dev), inode: String(inode(checkout).ino),
      lockDevice: String(inode(lock).dev), lockInode: String(inode(lock).ino) };
    const engine = writeFakeEngine(root, { "takeover-probe": { result }, "takeover-verify": { result },
      "takeover-writer-lock": { result } });
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-demo", project: "", role: "assistant" as const, assistant: "A-1" };
    const transport = fakeCodexTransport(), network = guardedFetch(transport.send);
    globalThis.fetch = network.fetch;
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(root, "agent"), model: "gpt-6.1-sol",
      capabilities: { catalog: 8, digest: "a".repeat(64), preset: "strict", enabled: ["codemode"] },
      ui: "plain", client: new CorralClient({ corral: engine.path, root, cwd, identity }), events: new EventLog(), network,
      modules: { auth: fixtureAuth, role: assistantRole(PACKAGE_ROOT), assetRoot: PACKAGE_ROOT },
      sessionManager: sdk.SessionManager.create(cwd, join(root, "sessions")) });
    await host.session.bindExtensions({});
    await host.startLocal();
    transport.script([call("request", "request_task_worktree", { task }),
      call("premature", "write", { path: source, content: "not allowed" }), { text: "Waiting." }]);
    await host.session.prompt("Edit this task after owner approval", { source: "extension" });
    expect(existsSync(source)).toBeFalse();
    const [question] = host.pendingOwnerQuestions();
    expect(question?.question).toContain(checkout);
    expect(question?.question).toContain(host.sessionId());
    expect(await host.replyToQuestion(CHOICE.session)).toBeFalse();
    tryWorktreeLock(lock, false).close(); // a pending question reserves no writer authority

    transport.script([call("direct", "write", { path: source, content: "first\n" }), call("nested", "eval_js", { code: `
      await tools.edit({path:${JSON.stringify(source)}, edits:[{oldText:'first',newText:'nested'}]});
      for (const path of ${JSON.stringify([link, sibling, protectedPath])}) {
        try { await tools.write({path, content:'denied'}); } catch {}
      }` }), { text: "Done." }]);
    await host.answerQuestion({ question: question!.id, option: CHOICE.session }, async () => {});
    await host.session.waitForIdle();
    expect(readFileSync(source, "utf8")).toBe("nested\n");
    expect(readFileSync(outside, "utf8")).toBe("outside");
    expect(readFileSync(protectedPath, "utf8")).toBe("metadata");
    expect(existsSync(sibling)).toBeFalse();
    expect(() => tryWorktreeLock(lock, false)).toThrow();
    await expect(taskWriterLock(checkout, engine.path, undefined)).rejects.toThrow();
    const alias = join(cwd, "alias.txt");
    symlinkSync(source, alias);
    await expect(taskDestinationLock(alias, engine.path, root)).rejects.toThrow();

    expect((await host.submitLocal({ origin: "owner", inputId: "inspect", busyPolicy: "steer", text: "/worktree-access" })).disposition).toBe("handled");
    expect((await host.submitLocal({ origin: "owner", inputId: "revoke", busyPolicy: "steer", text: "/worktree-access revoke" })).disposition).toBe("handled");
    mkdirSync(join(cwd, ".corral-next")); // a nearer board need not be the checkout's owner
    writeFileSync(join(cwd, ".corral-next", "corral.toml"), "");
    const writer = await taskWriterLock(checkout, engine.path, root);
    expect(writer).toBeDefined();
    expect(engine.calls().at(-1)?.slice(0, 3)).toEqual(["--root", root, "--json"]);
    writer?.close();
    transport.script([call("after", "write", { path: source, content: "denied" }), { text: "Finished." }]);
    await host.session.prompt("Try after hand-back", { source: "extension" });
    expect(readFileSync(source, "utf8")).toBe("nested\n");
    const states = host.session.sessionManager.getEntries().flatMap(entry =>
      entry.type === "custom" && entry.customType === WORKTREE_ACCESS_ENTRY ? [(entry.data as { state: string }).state] : []);
    expect(states).toEqual(["pending", "granted", "revoked"]);
  } finally {
    await host?.close(); globalThis.fetch = fetch;
    for (const key of Object.keys(process.env)) if (!(key in env)) delete process.env[key];
    Object.assign(process.env, env);
    rmSync(root, { recursive: true, force: true });
  }
}, 30_000);
