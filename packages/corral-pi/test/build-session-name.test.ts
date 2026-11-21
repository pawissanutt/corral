import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { InteractiveMode } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadSession, saveSession, sessionDir } from "../src/build/session-store.ts";
import type { BuildContext } from "../src/build/types.ts";
import { openSession } from "../src/host/lifecycle.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { authContext, HostSession } from "../src/host/session.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { RecordingTerminal } from "./support/panel-terminal.ts";
import { nextEvent, within } from "./support/wait.ts";

test("Pi /name overrides a Build's request title and reprojects session_info on resume", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "corral-build-session-name-"));
  const assets = buildFixtureInstall({ parent: cwd });
  const oldState = process.env.PI_CODING_AGENT_DIR;
  const oldFetch = globalThis.fetch;
  let host: HostSession | undefined;
  let mode: InteractiveMode | undefined;
  try {
    const agentDir = join(cwd, "agent"), transcripts = join(cwd, "sessions");
    process.env.PI_CODING_AGENT_DIR = agentDir;
    mkdirSync(agentDir);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const id = "B-rename", dir = sessionDir(cwd, id), journal = join(dir, "journal.json");
    const at = new Date().toISOString();
    saveSession(cwd, { version: 1, id, name: "rename", created: at, updated: at, worktree: cwd,
      repoRoot: cwd, branch: null, base: null, here: true, bare: true, repository: false });
    const identity = { store: "local", project: "", role: "build" as const };
    const modules = await loadModules(assets.dir, "build");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport unavailable");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const events = new EventLog(), client = new CorralClient({ corral: "", root: cwd, cwd, identity });
    const start = async (resume?: string) => {
      const build: BuildContext = { dir, corral: "", session: loadSession(cwd, id),
        settings: { maxParallel: 1, exitPolicy: "none", profiles: [] } };
      const sessionManager = openSession(sdk, cwd, transcripts, resume);
      const result = await HostSession.create({ sdk, identity, cwd, agentDir, model: "gpt-6.1-sol", ui: "plain",
        build, modules, client, events, network, sessionManager });
      await result.startLocal();
      result.installRemoteParts();
      return result;
    };
    const record = () => host!.remoteRegistry.get("record")!({});
    host = await start();
    const file = host.session.sessionFile!;
    saveSession(cwd, { ...loadSession(cwd, id), sessionFile: file });
    const terminal = new RecordingTerminal();
    mode = new sdk.InteractiveMode(host.runtime, { terminal, tuiMode: "fullscreen", readOnlyQueue: true });
    await mode.init();
    host.markAttached("interactive");
    void mode.run().catch(error => { throw error; });

    const renamed = Promise.withResolvers<string | undefined>();
    const unsubscribe = host.session.subscribe(event => {
      if (event.type === "session_info_changed") renamed.resolve(event.name);
    });
    try {
      for (const char of "/name Owner's chosen title") terminal.send(char);
      terminal.send("\r");
      expect(await within(renamed.promise, 10_000, "Pi session_info_changed")).toBe("Owner's chosen title");
    } finally { unsubscribe(); }
    expect(await record()).toMatchObject({ title: "Owner's chosen title" });
    const turnEnded = nextEvent(events, event => event.kind === "turn_ended");
    expect(await host.submitLocal({ text: "Fix the parser", busyPolicy: "follow_up", origin: "owner" }))
      .toMatchObject({ disposition: "started" });
    await turnEnded;
    const saved = JSON.parse(readFileSync(journal, "utf8"));
    expect(saved).toMatchObject({ title: "Fix the parser", sessionTitle: "Owner's chosen title", intent: [{ text: "Fix the parser" }] });
    expect(readFileSync(file, "utf8").trim().split("\n").map(line => JSON.parse(line)).findLast(entry => entry.type === "session_info"))
      .toMatchObject({ name: "Owner's chosen title" });
    expect(await record()).toMatchObject({ title: "Owner's chosen title" });
    mode.stop();
    mode = undefined;
    await host.close();

    // A transcript created before this projection existed still supplies the name on resume.
    delete saved.sessionTitle;
    writeFileSync(journal, JSON.stringify(saved));
    saveSession(cwd, { ...loadSession(cwd, id), title: "Launch title" });
    host = await start(file);
    expect(await record()).toMatchObject({ title: "Owner's chosen title" });
    expect(JSON.parse(readFileSync(journal, "utf8")).sessionTitle).toBe("Owner's chosen title");

    const bounded = "a b" + "😀".repeat(117);
    host.session.setSessionName("a\x1bb" + "😀".repeat(118) + " trailing");
    expect(await record()).toMatchObject({ title: bounded });
    expect(JSON.parse(readFileSync(journal, "utf8")).sessionTitle).toBe(bounded);
    host.session.setSessionName("");
    expect(await record()).toMatchObject({ title: "Launch title" });
    expect(JSON.parse(readFileSync(journal, "utf8")).sessionTitle).toBeUndefined();
  } finally {
    mode?.stop();
    await host?.close();
    globalThis.fetch = oldFetch;
    if (oldState === undefined) delete process.env.PI_CODING_AGENT_DIR; else process.env.PI_CODING_AGENT_DIR = oldState;
    rmSync(cwd, { recursive: true, force: true });
  }
}, 30_000);
