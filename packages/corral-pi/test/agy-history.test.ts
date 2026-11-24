import { expect, test } from "bun:test";
import type { Model } from "@earendil-works/pi-ai";
import { createAgentSession, createWriteTool, DefaultResourceLoader, SessionManager, SettingsManager } from "@earendil-works/pi-coding-agent";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { AgyOutput, driveAgy, type AgySession } from "../src/agy/stream.ts";
import { AgyToolLane } from "../src/agy/tool-lane.ts";
import type { FakeAgyCase } from "./fixtures/fake-agy.ts";

const posixTest = test.skipIf(process.platform === "win32");
const fixture = fileURLToPath(new URL("./fixtures/fake-agy.ts", import.meta.url));
const model: Model<string> = {
  id: "gemini-4-neon-high", name: "Fake Gemini", api: "agy-acp", provider: "openai", baseUrl: "https://unused.invalid",
  reasoning: true, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1_000, maxTokens: 100,
};
const shellQuote = (arg: string) => `'${arg.replaceAll("'", "'\\''")}'`;
const marker = (id: string) => `[[agy:${id}]]`;
const remember = (id: string) => `remember ${marker(id)}`;
const recall = "Which markers are visible?";
// A valid MCP response that exceeds the old 1 MiB ACP prompt frame limit.
const largeToolResult = "x".repeat(1024 * 1024) + marker("tool_result");

type Wire = { method?: string; params?: unknown };
type Prompt = { sessionId: string; prompt: { type: string; text: string }[] };

async function withHistoryPeer(scenario: Extract<FakeAgyCase, "history" | "mcp-history">,
  run: (peer: ReturnType<typeof historyPeer>) => Promise<void>) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "corral-agy-history-")));
  const closers: (() => Promise<void>)[] = [];
  try {
    await run(historyPeer(root, scenario, closers));
  } finally {
    try { await Promise.all(closers.map(close => close())); }
    finally { rmSync(root, { recursive: true, force: true }); }
  }
}

function historyPeer(root: string, scenario: "history" | "mcp-history", closers: (() => Promise<void>)[]) {
  const cwd = join(root, "checkout"), home = join(root, "agy-home"), agentDir = join(root, "pi");
  mkdirSync(cwd);
  mkdirSync(home, { mode: 0o700 });
  const executable = join(root, "fake-agy");
  // Use this Bun executable; the child has a stripped PATH and private HOME.
  const command = [process.execPath, fixture, scenario, root].map(shellQuote).join(" ");
  writeFileSync(executable, `#!/bin/sh\nexec ${command} "$@"\n`, { mode: 0o700 });
  const launch = { cwd, home, executable, setupTimeoutMs: 1_500, turnTimeoutMs: 2_000 };
  const writes: string[] = [];
  const wire = (): Wire[] => readFileSync(join(root, "wire.ndjson"), "utf8").trim().split("\n").map(line => JSON.parse(line));
  return {
    cwd, launch, writes,
    methods: () => wire().flatMap(packet => packet.method ?? []),
    prompts: () => wire().filter(packet => packet.method === "session/prompt").map(packet => packet.params as Prompt),
    mcp: (): { label: string; result: unknown }[] => readFileSync(join(root, "mcp.ndjson"), "utf8").trim().split("\n")
      .map(line => JSON.parse(line)),
    async open(manager = SessionManager.create(cwd, join(root, "sessions"))) {
      const withWrite = scenario === "mcp-history";
      const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false, keepRecentTokens: 1 }, retry: { enabled: false } });
      const loader = new DefaultResourceLoader({ cwd, agentDir, settingsManager, systemPrompt: "Synthetic fixture instructions.",
        noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
        extensionFactories: [pi => {
          pi.on("session_before_compact", ({ preparation }) => ({ compaction: {
            summary: marker("summary"), firstKeptEntryId: preparation.firstKeptEntryId, tokensBefore: preparation.tokensBefore,
          } }));
          if (!withWrite) return;
          const write = createWriteTool(cwd);
          pi.registerTool({ ...write, async execute(id, input, signal, update) {
            writes.push(input.path);
            const result = await write.execute(id, input, signal, update);
            // A result-only marker proves the MCP output, not just the tool input, was seeded.
            return { ...result, content: [...result.content, { type: "text", text: largeToolResult }] };
          } });
        }],
      });
      await loader.reload();
      const { session } = await createAgentSession({ cwd, agentDir, model, settingsManager,
        resourceLoader: loader, tools: withWrite ? ["write"] : [], sessionManager: manager });
      const lane = new AgyToolLane(launch, session.agent);
      let closed = false;
      const close = async () => {
        if (closed) return;
        closed = true;
        try { await lane.close(); }
        finally { session.dispose(); }
      };
      closers.push(close);
      session.agent.streamFunction = lane.stream;
      await session.bindExtensions({ onError: error => { throw error.error; } });
      // Pi preflights auth; no built-in stream or real key is used.
      await session.modelRuntime.setRuntimeApiKey(model.provider, "fixture-not-a-secret");
      return {
        session, manager, close,
        async ask(text: string) {
          await session.prompt(text);
          const last = manager.getLeafEntry();
          if (last?.type !== "message" || last.message.role !== "assistant" || last.message.stopReason !== "stop") {
            throw new Error(`Expected completed Pi turn: ${JSON.stringify(last)}`);
          }
          return last.message.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n");
        },
      };
    },
  };
}

function priorText(prompt: Prompt, latest: string): string {
  expect(prompt.prompt.at(-1)).toEqual({ type: "text", text: latest });
  expect(prompt.prompt.every(block => block.type === "text")).toBe(true);
  return prompt.prompt.slice(0, -1).map(block => block.text).join("\n");
}

posixTest("agy reuses tool-less turns but reseeds only the selected Pi branch after rewind", async () => {
  await withHistoryPeer("history", async peer => {
    const parent = await peer.open();
    expect(await parent.ask(remember("retained"))).toContain(marker("reply_retained"));
    const target = parent.manager.getLeafId()!;
    expect(await parent.ask(remember("discarded"))).toContain(marker("reply_discarded"));
    const discarded = parent.manager.getLeafId()!;
    expect(discarded).not.toBe(target);
    expect(await parent.session.navigateTree(target)).toMatchObject({ cancelled: false });
    expect(parent.manager.getLeafId()).toBe(target);

    const recalled = await parent.ask(recall); // Same Pi agent and live lane.
    const [first, second, rewound] = peer.prompts();
    expect(second!.sessionId).toBe(first!.sessionId);
    expect(priorText(second!, remember("discarded"))).toBe("");
    expect(rewound!.sessionId).not.toBe(first!.sessionId);
    const seed = priorText(rewound!, recall);
    expect(seed).toContain(marker("retained"));
    expect(seed).toContain(marker("reply_retained"));
    expect(seed).not.toContain(marker("discarded"));
    expect(seed).not.toContain(marker("reply_discarded"));
    expect(recalled).toContain("retained");
    expect(recalled).not.toContain("discarded");
    expect(peer.methods().filter(method => method === "session/new")).toHaveLength(2);
    expect(peer.methods()).not.toContain("session/load");
  });
});

posixTest("agy seeds a fresh session from Pi's persisted transcript after interruption and restart", async () => {
  await withHistoryPeer("history", async peer => {
    const original = await peer.open();
    expect(await original.ask(remember("before_interrupt"))).toContain(marker("reply_before_interrupt"));
    // Abort only when the ACP peer confirms it is handling the second prompt. No timing sleeps.
    original.session.agent.onProviderStreamEvent = wire => {
      if (JSON.stringify(wire).includes("Interruption barrier")) original.session.agent.abort();
    };
    await original.session.prompt(`interrupt ${marker("interrupted")}`);
    expect(original.manager.getLeafEntry()).toMatchObject({ type: "message", message: { role: "assistant", stopReason: "aborted" } });
    expect(peer.methods()).toContain("session/cancel");
    const file = original.manager.getSessionFile();
    if (!file || !existsSync(file)) throw new Error("Pi did not persist the interrupted session");
    await original.close();

    const reopened = await peer.open(SessionManager.open(file));
    expect(await reopened.ask(recall)).toContain("before_interrupt");
    const [, interrupted, restarted] = peer.prompts();
    expect(restarted!.sessionId).not.toBe(interrupted!.sessionId);
    const seed = priorText(restarted!, recall);
    expect(seed).toContain(marker("before_interrupt"));
    expect(seed).toContain(marker("reply_before_interrupt"));
    expect(peer.methods()).not.toContain("session/load");
  });
});

posixTest("agy rebuilds on Pi compaction and reselects the exact model after a switch", async () => {
  await withHistoryPeer("history", async peer => {
    const { session, ask } = await peer.open();
    await ask(remember("summarized"));
    await ask(remember("kept_recent"));
    await session.compact();
    expect(await ask(recall)).not.toContain("summarized");
    await session.setModel({ ...model, id: "gemini-4-neon" });
    expect(await ask(recall)).toContain("summary");
    const [first, , compacted, switched] = peer.prompts();
    expect(compacted!.sessionId).not.toBe(first!.sessionId);
    expect(priorText(compacted!, recall)).toContain(marker("summary"));
    expect(priorText(compacted!, recall)).not.toContain(marker("summarized"));
    expect(switched!.sessionId).not.toBe(compacted!.sessionId);
    expect(peer.methods().slice(-3)).toEqual(["session/new", "session/set_model", "session/prompt"]);
  });
});

posixTest("agy drops an idle session that emits unrecorded conversation updates", async () => {
  await withHistoryPeer("history", async peer => {
    const { ask } = await peer.open();
    await ask(remember("idle_drift"));
    const recalled = await ask(recall);
    const [first, second] = peer.prompts();
    expect(second!.sessionId).not.toBe(first!.sessionId);
    expect(recalled).toContain("reply_idle_drift");
    expect(recalled).not.toContain("late_unrecorded");
  });
});

posixTest("agy reseeds before dispatch when the resident closes during Pi's payload hook", async () => {
  await withHistoryPeer("history", async peer => {
    const instructions = "Synthetic fixture instructions.";
    const first = new AgyOutput(model);
    let resident: AgySession | undefined;
    try {
      await driveAgy(peer.launch, model, { instructions, prompt: remember("retained") }, first, undefined, undefined, {
        retain: session => { resident = session; return true; },
      });
      expect((await first.current.stream.result()).stopReason).toBe("stop");
      if (!resident) throw new Error("Expected a resident peer");
      const old = resident;
      const second = new AgyOutput(model);
      const seed = [{ type: "text" as const, text: remember("retained") }, { type: "text" as const, text: recall }];
      const attempts: string[] = [];
      await driveAgy(peer.launch, model, { instructions, prompt: recall }, second, { onPayload: async wire => {
        const payload = wire as Prompt;
        attempts.push(payload.sessionId);
        // Hold the payload hook until the old peer has closed. Its edited delta must not be sent.
        if (payload.sessionId === old.sessionId) {
          await old.peer.close();
          return { ...payload, prompt: [{ type: "text", text: remember("abandoned_payload") }] };
        }
      } }, undefined, { reuse: { session: old, seed: () => seed } });
      const response = await second.current.stream.result();
      expect(response.stopReason).toBe("stop");
      expect(response.content).toMatchObject([{ type: "text", text: "Seen: retained" }]);
      const [oldPrompt, freshPrompt] = peer.prompts();
      expect(freshPrompt!.sessionId).not.toBe(oldPrompt!.sessionId);
      expect(freshPrompt!.prompt).toEqual(seed);
      expect(attempts).toEqual([oldPrompt!.sessionId, freshPrompt!.sessionId]);
      expect(peer.prompts()).toHaveLength(2);
    } finally {
      await resident?.peer.close();
    }
  });
});

posixTest("agy child agent cannot recall a live parent's state even with identical launch configuration", async () => {
  await withHistoryPeer("history", async peer => {
    const parent = await peer.open();
    expect(await parent.ask(remember("parent_private"))).toContain(marker("reply_parent_private"));
    const child = await peer.open(); // New manager, agent and lane; cwd, home and executable are identical.
    expect(await child.ask(recall)).not.toContain("parent_private");
    const [parentPrompt, childPrompt] = peer.prompts();
    expect(childPrompt!.sessionId).not.toBe(parentPrompt!.sessionId);
    expect(priorText(childPrompt!, recall)).not.toContain("parent_private");
    expect(peer.methods().filter(method => method === "session/new")).toHaveLength(2);
    expect(peer.methods()).not.toContain("session/load");
  });
});

posixTest("agy seeds a fresh prompt with Pi's large recorded MCP result without replaying the write", async () => {
  await withHistoryPeer("mcp-history", async peer => {
    const { ask, manager } = await peer.open();
    await ask("exercise write");
    const results = () => manager.getBranch().flatMap(entry =>
      entry.type === "message" && entry.message.role === "toolResult" ? [entry.message] : []);
    const written = results();
    expect(written).toMatchObject([{ toolName: "write", isError: false }]);
    expect(written[0]!.content.some(block => block.type === "text" && block.text === largeToolResult)).toBe(true);
    expect(peer.mcp()).toMatchObject([
      { label: "list" }, { label: "history-write", result: { result: { content: written[0]!.content, isError: false } } },
    ]);
    const recalled = await ask(recall);
    expect(peer.writes).toEqual(["history.txt"]);
    expect(readFileSync(join(peer.cwd, "history.txt"), "utf8")).toBe("written once");
    expect(results()).toEqual(written);
    const [first, second] = peer.prompts();
    // The MCP wire lacks prompt correlation, so neither session nor grants may be reused.
    expect(second!.sessionId).not.toBe(first!.sessionId);
    expect(peer.methods().filter(method => method === "session/new")).toHaveLength(2);
    expect(peer.methods()).not.toContain("session/load");
    expect(priorText(second!, recall)).toContain(marker("tool_result"));
    expect(recalled).toContain("tool_result");
  });
});
