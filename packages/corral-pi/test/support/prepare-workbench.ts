// Private offline fixture installer for the real production host/main.ts PTY check.
import { mkdirSync, readFileSync, writeFileSync, copyFileSync } from "node:fs";
import { join } from "node:path";
import { buildFixtureInstall } from "./fixture-install.ts";
import { writeFakeCorral } from "./fake-corral.ts";

const work = process.argv[2];
if (!work) throw new Error("private fixture directory required");
const assets = buildFixtureInstall({ parent: work, isolatePiState: true });
const corral = writeFakeCorral(work);
copyFileSync(new URL("../fixtures/images/zoom.png", import.meta.url), join(work, "zoom.png"));
const path = join(assets.dir, "src/roles/fixture-role.ts");
const source = readFileSync(path, "utf8").replace('corralOps: ["hello", "event", "receipt_lookup"]', 'corralOps: ["hello", "event", "receipt_lookup", "job", "job_monitor"]');
const marker = "    extend(pi) {";
if (source.split(marker).length !== 2) throw new Error("fixture extension hook changed");
writeFileSync(path, source.replace(marker, `${marker}
      pi.on("ui_prompt_end", async () => {
        const { appendFileSync } = await import("node:fs");
        appendFileSync(${JSON.stringify(join(work, "ui-events.jsonl"))}, "ended\\n");
      });
      pi.registerCommand("fixture-workbench-image", {
        description: "Owned offline native image scenario",
        handler: async () => {
          transport().script([{ toolCalls: [{ callId: "image-read", name: "read", arguments: { path: "${work}/zoom.png" } }] }, { text: "IMAGE_READY" }]);
        }
      });
      pi.registerCommand("fixture-workbench-jobs", {
        description: "Owned offline background-job scenario",
        handler: async () => {
          transport().script([
            { toolCalls: [{ callId: "background-start", name: "job_start", arguments: { command: "printf 'BUILD_WAIT\\n'; IFS= read -r ready < ${work}/monitor-gate; printf 'MONITOR_READY\\n'; IFS= read -r done < ${work}/exit-gate; exit 7" } }] },
            { toolCalls: [{ callId: "background-monitor", name: "corral_job_monitor", arguments: { job: "j1-1", on: "line", pattern: "MONITOR_READY", timeout_secs: 60 } }] },
            { text: "BACKGROUND_STARTED" }
          ]);
        }
      });
      pi.registerCommand("fixture-panel-stream", {
        description: "Owned offline read tool call, then a reply held after its first delta",
        handler: async () => {
          const { readFile } = await import("node:fs/promises");
          transport().script([
            { toolCalls: [{ callId: "panel-read", name: "read", arguments: { path: "${work}/tool-output.txt" } }] },
            { chunks: ["STREAM_PART_ONE", " STREAM_PART_TWO"], afterFirstChunk: () => readFile("${work}/stream-gate").then(() => undefined) }
          ]);
        }
      });
      pi.registerCommand("fixture-native-confirm", {
        description: "Owned native UI cancellation test",
        handler: async (_args, ctx) => {
          const answer = await ctx.ui.confirm("Native permission fixture", "Cancel this isolated test.");
          ctx.ui.notify(answer ? "NATIVE_ALLOWED" : "NATIVE_CANCELLED");
        },
      });`));
for (const name of ["run", "sessions", "home", "config", "data", "state", "cache"]) mkdirSync(join(work, name), { recursive: true });
process.stdout.write(JSON.stringify({ assets: assets.dir, corral: corral.path }));
