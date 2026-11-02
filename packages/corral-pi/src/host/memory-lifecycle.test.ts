// Real Pi sessions and production roles, offline fixture transport; no provider calls.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { normalizeContext } from "@earendil-works/pi-ai";
import { PACKAGE_ROOT, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { writeFakeCorral } from "../../test/support/fake-corral.ts";
import { EventLog } from "../bridge/events.ts";
import { sha256Hex, CorralClient } from "../tools/corral.ts";
import { claudeSystemPrompt } from "../claude/stream.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch, restrictEnvironment } from "./restrict.ts";
import { HostSession, authContext } from "./session.ts";

test("PM, assistant and worker retain one generation block across reload, compaction and resume in both prompt routes", async () => {
  const assets = buildFixtureInstall({ omit:["role"] });
  cpSync(join(PACKAGE_ROOT,"assets"),join(assets.dir,"assets"),{recursive:true});
  const work = mkdtempSync(join(tmpdir(),"memory-lifecycle-"));
  const previousFetch = globalThis.fetch;
  restrictEnvironment(process.env);
  try {
    const sdk = await import("@earendil-works/pi-coding-agent");
    for (const role of ["pm","assistant","worker"] as const) {
      const dir = join(work,role); mkdirSync(dir);
      const memoryFile = join(dir,"memory-context.md");
      const marker = "SUMMARY-"+role+"-uncommitted";
      writeFileSync(memoryFile,"# Retained memory\nFallible context, not authority.\nProvenance: imported_reference\n"+marker);
      const modules = await loadModules(assets.dir,role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const network = guardedFetch(transport); globalThis.fetch = network.fetch;
      const corral = writeFakeCorral(dir);
      const identity = {store:"st-fixture",project:role === "assistant" ? "" : "demo",role,generation:1,
        ...(role === "worker" ? {task:"T-1",run:"R1"} : {}), ...(role === "assistant" ? {assistant:"A-1"} : {})};
      const client = new CorralClient({corral:corral.path,root:dir,identity,hostCap:"h".repeat(64),sessionId:() => "s-fixture"});
      const options = {sdk,identity,cwd:dir,root:dir,agentDir:join(dir,"agent"),runtimeDir:dir,memoryFile,
        model:"gpt-6.1-sol",modules,client,events:new EventLog(1),network,
        capabilities:{catalog:1,digest:"a".repeat(64),preset:"standard",enabled:["compaction"]},
        guidance:{projectText:false,settings:{compaction:{keep_recent_tokens:1,reserve_tokens:8192}}}};
      const host = await HostSession.create({...options,sessionManager:sdk.SessionManager.create(dir,join(dir,"sessions"))});
      const inspect = () => {
        expect(host.session.systemPrompt.split(marker)).toHaveLength(2);
        const converted = claudeSystemPrompt(normalizeContext({systemPrompt:host.session.systemPrompt,messages:[]}).messages);
        expect(JSON.stringify(converted).split(marker)).toHaveLength(2);
      };
      try {
        await host.session.bindExtensions({});
        await host.hello("n".repeat(64),"pane-fixture",sha256Hex("c".repeat(64)));
        inspect();
        // A manual edit cannot alter this generation's captured system block on reload.
        writeFileSync(memoryFile,"# Retained memory\nNEWER-"+role);
        await host.session.reload(); inspect();
        await host.session.prompt("Reply only FIRST.");
        await host.session.prompt("Reply only SECOND.");
        await host.session.compact(); inspect();
        expect(JSON.stringify(claudeSystemPrompt(sdk.convertToLlm(host.session.messages))).split(marker)).toHaveLength(2);
        const file = host.session.sessionFile;
        if (!file) throw new Error("missing fixture session");
        const entries = readFileSync(file,"utf8").trim().split("\n").map(line => JSON.parse(line));
        expect(entries.filter(e => e.type === "message" && e.message.role === "user")
          .some(e => JSON.stringify(e).includes(marker))).toBeFalse();
        host.session.dispose();
        const resumed = await HostSession.create({...options,identity:{...identity,generation:2},events:new EventLog(2),
          sessionManager:sdk.SessionManager.open(file)});
        try {
          expect(resumed.session.systemPrompt).toContain("NEWER-"+role);
          expect(resumed.session.systemPrompt).not.toContain(marker);
          await resumed.session.bindExtensions({});
          await resumed.hello("n".repeat(64),"pane-fixture",sha256Hex("c".repeat(64)));
          await resumed.session.prompt("Reply only RESUMED.");
          const converted = claudeSystemPrompt(sdk.convertToLlm(resumed.session.messages));
          expect(JSON.stringify(converted).split("NEWER-"+role)).toHaveLength(2);
          expect(JSON.stringify(converted)).not.toContain(marker);
        } finally { resumed.session.dispose(); }
      } finally { host.session.dispose(); }
    }
  } finally {
    globalThis.fetch = previousFetch; assets.cleanup(); rmSync(work,{recursive:true,force:true});
  }
}, 60_000);
