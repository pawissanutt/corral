// Offline comparison of exact Git-base replay code against this candidate. No SDK, credentials,
// sockets or provider calls. Reusable serialized bytes are a proxy, NOT provider token/cache telemetry.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { transpileModule, ModuleKind, ScriptTarget } from "typescript";
import type { AssistantMessage, Message } from "@earendil-works/pi-ai";
import { buildPromptBlocks, type PromptBlock } from "../../src/claude/prompt.ts";
import { ReplayCache } from "../../src/claude/cache.ts";

interface Prepared { blocks: PromptBlock[]; commit(): void }
interface Cache { prepare(blocks: PromptBlock[], scope: string): Prepared }
interface Implementation {
  buildPromptBlocks: typeof buildPromptBlocks;
  ReplayCache: new () => Cache;
}

const base = process.argv[2];
if (!base || !/^[a-f0-9]{7,40}$/.test(base)) throw new Error("usage: bun test/support/claude-cache-benchmark.ts <base-sha>");
const root = fileURLToPath(new URL("../../../../", import.meta.url));
const transpile = (source: string) => transpileModule(source, {
  compilerOptions: { module: ModuleKind.ESNext, target: ScriptTarget.ES2022 },
}).outputText;
async function baselineModule(file: string) {
  const source = execFileSync("git", ["show", `${base}:packages/corral-pi/src/claude/${file}.ts`],
    { cwd: root, encoding: "utf8", timeout: 5000, maxBuffer: 256 * 1024 })
    .replace('from "./tools.ts"', `from ${JSON.stringify(new URL("../../src/claude/tools.ts", import.meta.url).href)}`);
  return import(`data:text/javascript;base64,${Buffer.from(transpile(source)).toString("base64")}`);
}
const oldPrompt: Pick<Implementation, "buildPromptBlocks"> = await baselineModule("prompt");
const oldCache: Pick<Implementation, "ReplayCache"> = await baselineModule("cache");
const before: Implementation = { ...oldPrompt, ...oldCache };
const after: Implementation = { buildPromptBlocks, ReplayCache };

function prefix(blocks: readonly PromptBlock[], end: number): string {
  return JSON.stringify(blocks.slice(0, end + 1).map(block => block.type === "text" ? { type: "text", text: block.text } : block));
}
const hash = (text: string) => createHash("sha256").update(text).digest("hex");
const marker = (blocks: readonly PromptBlock[]) => blocks.findLastIndex(block => block.type === "text" && !!block.cache_control);
const user = (text: string): Message => ({ role: "user", content: text, timestamp: 0 });
function assistant(turn: number): AssistantMessage {
  return { role: "assistant", content: [{ type: "text", text: `Inspect batch ${turn}.` }], api: "claude-agent-sdk",
    provider: "anthropic", model: "synthetic", stopReason: "toolUse", timestamp: 0,
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
}
function measure(implementation: Implementation, batch: number, images: boolean, reset: "none" | "compact" | "restart") {
  let cache = new implementation.ReplayCache();
  let messages: Message[] = [user("Bounded synthetic task context. ".repeat(256))];
  const writes = new Set<string>();
  let readBytes = 0, sentBytes = 0, worstHistoryLag = 0, tailBytes = 0;
  for (let turn = 0; turn < 6; turn++) {
    if (turn === 3 && reset === "compact") messages = [user("Canonical summary; old detailed history was compacted.")];
    if (turn === 3 && reset === "restart") cache = new implementation.ReplayCache();
    messages.push(assistant(turn));
    for (let item = 0; item < batch; item++) messages.push({ role: "toolResult", toolName: "read",
      toolCallId: `turn-${turn}-result-${item}`, isError: false, timestamp: 0, content: [
        { type: "text", text: `Result ${turn}/${item}: ${"bounded evidence ".repeat(32)}` },
        ...(images ? [{ type: "image" as const, mimeType: "image/png", data: "aW1hZ2U=" }] : []),
      ] });
    const rendered = implementation.buildPromptBlocks(messages, new Map());
    const prepared = cache.prepare(rendered, "offline-session");
    const at = marker(prepared.blocks);
    let hit = -1;
    for (let index = at; index >= Math.max(0, at - 19); index--) {
      if (writes.has(hash(prefix(prepared.blocks, index)))) { hit = index; break; }
    }
    if (hit >= 0) readBytes += Buffer.byteLength(prefix(prepared.blocks, hit));
    sentBytes += Buffer.byteLength(prefix(prepared.blocks, prepared.blocks.length - 1));
    const end = marker(rendered);
    worstHistoryLag = Math.max(worstHistoryLag, end - at);
    tailBytes += Buffer.byteLength(prefix(rendered, end)) - Buffer.byteLength(prefix(rendered, at));
    if (at >= 0) writes.add(hash(prefix(prepared.blocks, at)));
    prepared.commit();
  }
  return { modeledReplayReadPercent: Number((100 * readBytes / sentBytes).toFixed(2)),
    historyBeyondWriteBytes: tailBytes, worstHistoryLagBlocks: worstHistoryLag, sentReplayJsonBytes: sentBytes };
}
const scenarios = [
  { name: "serial-text", batch: 1, images: false, reset: "none" },
  { name: "parallel-text", batch: 25, images: false, reset: "none" },
  { name: "parallel-images", batch: 25, images: true, reset: "none" },
  { name: "compaction", batch: 25, images: false, reset: "compact" },
  { name: "restart", batch: 25, images: false, reset: "restart" },
] as const;
console.log(JSON.stringify({ base, requestsPerScenario: 6, network: "none", providerCalls: 0,
  metric: "modeled replay JSON bytes; excludes SDK system/tools, actual tokenization, TTL and provider behavior",
  scenarios: scenarios.map(scenario => ({ scenario: scenario.name,
    before: measure(before, scenario.batch, scenario.images, scenario.reset),
    after: measure(after, scenario.batch, scenario.images, scenario.reset) })) }, null, 2));
