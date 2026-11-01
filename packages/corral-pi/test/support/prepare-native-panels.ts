// Private offline fixture installer for the real production host/main.ts PTY check.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildFixtureInstall } from "./fixture-install.ts";
import { writeFakeCorral } from "./fake-corral.ts";

const work = process.argv[2];
if (!work) throw new Error("private fixture directory required");
const assets = buildFixtureInstall({ parent: work, isolatePiState: true });
const corral = writeFakeCorral(work);
const path = join(assets.dir, "src/roles/fixture-role.ts");
const source = readFileSync(path, "utf8");
const marker = "    extend(pi) {";
if (source.split(marker).length !== 2) throw new Error("fixture extension hook changed");
writeFileSync(path, source.replace(marker, `${marker}
      pi.on("ui_prompt_end", async () => {
        const { appendFileSync } = await import("node:fs");
        appendFileSync(${JSON.stringify(join(work, "ui-events.jsonl"))}, "ended\\n");
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
