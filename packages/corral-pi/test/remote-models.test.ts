import { expect, test } from "bun:test";
import { join } from "node:path";
import { RemoteOwner } from "../src/host/remote-owner.ts";
import { installRemote } from "../src/host/remote/index.ts";
import type { SessionModels } from "../src/host/remote/models.ts";
import { SessionRecord } from "../src/host/session-record.ts";
import { registerCommands, typedSessionAction } from "../src/host/remote/commands.ts";
import { RemoteRegistry } from "../src/host/remote/registry.ts";
import { codeHost } from "./support/code-host.ts";

test("a refused parent-model preflight leaves the current and default model unchanged", async () => {
  const fx = await codeHost("assistant", { enabled: [] });
  try {
    const { host } = fx, { session } = host;
    const record = new SessionRecord(fx.cwd, "A-1", "assistant", fx.cwd);
    const owner = new RemoteOwner(host, fx.events, record, session.sessionFile!, join(fx.cwd, "runtime"));
    installRemote(host, owner, fx.cwd);
    const models = host.remoteRegistry.get("models")!;
    const first = await models({}) as SessionModels;
    expect(first.current).toBe(`${session.model?.provider}/${session.model?.id}`);
    expect(first.default).toBe(first.current);
    expect(first.models.find(model => model.id === first.current)?.effort_levels).toEqual(session.getAvailableThinkingLevels());
    const alternative = first.models.find(model => model.id !== first.current);
    if (!alternative) throw new Error("the fixture requires two selectable models");
    session.modelRuntime.getAuth = async () => undefined;
    const leaf = session.sessionManager.getLeafId();
    await expect(owner.setModel({ model: alternative.id })).rejects.toThrow();
    expect(session.sessionManager.getLeafId()).toBe(leaf);
    expect(await models({})).toEqual(first);
  } finally { await fx.close(); }
});

test("a typed compact command calls the session action instead of becoming a model prompt", async () => {
  const registry = new RemoteRegistry(), instructions: (string | undefined)[] = [];
  registerCommands(registry, { list: () => ({ commands: [] }), ready: () => true, async compact(text) {
    instructions.push(text);
    return { tokensBefore: 1000 };
  } });
  expect(typedSessionAction(registry, "/compactness test")).toBeUndefined();
  await typedSessionAction(registry, "/compact focus on the decision");
  expect(instructions).toEqual(["focus on the decision"]);
});
