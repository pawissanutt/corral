// Exercise the production HostSession and roles with deterministic provider/UI boundaries.
// The fake receipt client persists the host's journal events, as the real assistant backend does.
import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { pmRole } from "../src/roles/pm/index.ts";
import type { HostIdentity } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";

const FAKE = `#!/usr/bin/env bun
import { appendFileSync, existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
const dir = dirname(process.argv[1]);
if (process.argv[2] === "assistant") { process.stdout.write(JSON.stringify({ container: { version: 1 }, workflow: { workflow: null, phase: "ordinary" } })); process.exit(0); }
const envelope = JSON.parse(readFileSync(0, "utf8"));
let result = envelope.op === "assistant_plan" && envelope.payload.action === "read"
  ? { workflow: null, phase: "ordinary" }
  : envelope.payload.kind === "budget" ? { admitted: true, exhausted: false } : { op: envelope.op };
if (envelope.payload.kind === "owner_input") {
  const path = join(dir, "journal.jsonl");
  const n = existsSync(path) ? readFileSync(path, "utf8").trim().split("\\n").length + 1 : 1;
  const id = "A-1/g1/i" + n;
  appendFileSync(path, JSON.stringify({ id, ...envelope.payload }) + "\\n");
  result = { kind: "owner_input", input: id };
}
process.stdout.write(JSON.stringify({ version: 1, request_id: envelope.request_id, op: envelope.op,
  state: "committed", result, at: new Date().toISOString() }) + "\\n");
`;

for (const role of ["assistant", "pm"] as const) {
  test(`${role} host records completed dialog answers as user messages and not cancellations`, async () => {
    const assets = buildFixtureInstall();
    const work = mkdtempSync(join(tmpdir(), `ask-user-${role}-`));
    const cwd = join(work, "ws");
    mkdirSync(cwd, { recursive: true });
    const corral = join(work, "corral");
    writeFileSync(corral, FAKE, { mode: 0o700 });
    const originalFetch = globalThis.fetch;
    const originalEnv = { ...process.env };
    let host: HostSession | undefined;
    try {
      restrictEnvironment(process.env);
      const sdk = await import("@earendil-works/pi-coding-agent");
      const modules = await loadModules(assets.dir, role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport);
      globalThis.fetch = network.fetch;
      const identity: HostIdentity = { store: "st-demo", project: role === "pm" ? "demo" : "", role,
        ...(role === "assistant" ? { assistant: "A-1" } : {}), generation: 1 };
      const client = new CorralClient({ corral, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
      host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
        capabilities: { catalog: 8, digest: "b".repeat(64), preset: "standard", enabled: ["ask_user"] },
        ui: "plain", client, events: new EventLog(1), network,
        modules: { ...modules, role: role === "assistant" ? assistantRole(PACKAGE_ROOT) : pmRole(PACKAGE_ROOT) },
        sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")) });
      let cancel = false;
      const seen: unknown[] = [];
      const ui = { select: async (title: string, options: string[]) => {
        seen.push([title, options]); return cancel ? undefined : "Two";
      }, input: async () => "Extra detail" };
      await host.session.bindExtensions({ uiContext: ui as unknown as ExtensionUIContext });
      await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
      const live = host;
      const userText = () => live.session.messages.filter((m) => m.role === "user")
        .map((m) => typeof m.content === "string" ? m.content : m.content.map((p) => p.type === "text" ? p.text : "").join(""));
      const journal = () => existsSync(join(work, "journal.jsonl"))
        ? readFileSync(join(work, "journal.jsonl"), "utf8").trim().split("\n").map((line) => JSON.parse(line)) : [];
      const ask = (id: string) => ({ toolCalls: [{ callId: id, name: "ask_user", arguments: {
        questions: [{ question: "Which?", options: ["One", "Two"], freeText: true }],
      } }] });
      fixtureTransport().script([ask("answer"), { text: "Understood." }, { text: "Noted." }]);
      await live.session.prompt("Ask a choice", { source: "extension" });
      await live.drained();
      const answer = "Which?: Two\nExtra detail";
      const tagged = role === "assistant" ? `${answer}\n\n[owner input A-1/g1/i1]` : answer;
      expect(seen).toEqual([["Which?", ["One", "Two", "Other (type an answer)"]]]);
      expect(userText()).toContain(tagged);
      expect(JSON.stringify(live.session.sessionManager.getEntries())).toContain(tagged.replaceAll("\n", "\\n"));
      expect(journal()).toEqual(role === "assistant" ? [{ id: "A-1/g1/i1", kind: "owner_input", source: "dialog",
        text: answer, sha256: sha256Hex(answer) }] : []);
      const before = userText();
      const previousJournal = journal();
      cancel = true;
      fixtureTransport().script([ask("cancel"), { text: "No answer." }]);
      await live.session.prompt("Ask again", { source: "extension" });
      await live.drained();
      expect(userText().slice(before.length)).toEqual(["Ask again"]);
      expect(live.session.messages.filter((m) => m.role === "toolResult" && m.toolCallId.startsWith("cancel"))
        .every((m) => !JSON.stringify(m).includes("[owner input"))).toBeTrue();
      expect(journal()).toEqual(previousJournal);
      const result = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.startsWith("cancel"));
      expect(JSON.stringify(result)).toContain("no answer");
    } finally {
      await host?.close();
      globalThis.fetch = originalFetch;
      for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
      Object.assign(process.env, originalEnv);
      assets.cleanup();
      rmSync(work, { recursive: true, force: true });
    }
  }, 120_000);
}
