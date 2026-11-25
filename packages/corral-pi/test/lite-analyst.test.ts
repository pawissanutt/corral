import { expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const entry = fileURLToPath(new URL("../src/lite-analyst/main.ts", import.meta.url));
const assets = new URL("../../../docs/contracts/lite-analyst-v1/", import.meta.url);
const read = (file: string) => readFileSync(new URL(file, assets), "utf8");
type Descriptor = { name: string; description: string; inputSchema: object };
const vector: { model: string; messages: { role: string; content: string }[]; tools: Descriptor[] } =
  JSON.parse(Buffer.from(JSON.parse(read("vectors.json")).model_request_b64, "base64").toString("utf8"));
// C1 changes the T-2069 vector's transport, not its exact prompt, question or registry.
const pinnedTurn = {
  model: vector.model,
  stream: true,
  messages: vector.messages,
  tools: vector.tools.map(({ name, description, inputSchema }) => ({
    type: "function", function: { name, description, parameters: inputSchema },
  })),
};
const start = {
  type: "start",
  question: vector.messages[1]!.content,
  prompt: read("synthetic-prompt.txt"),
  tools: JSON.parse(read("tool-policy.json")).model_tools as Descriptor[],
};

type Pi = {
  send: (frame: object) => void;
  raw: (bytes: string | Buffer) => void;
  receive: () => Promise<unknown>;
  expectExit: (code: number) => Promise<void>;
};

async function withPi(check: (pi: Pi) => Promise<void>) {
  const cwd = mkdtempSync(join(tmpdir(), "corral-pi-lite-analyst-test-"));
  // Visible canaries outside Git: the narrow host must neither load extensions nor fold repo/home text into the prompt.
  const extensions = join(cwd, ".pi", "agent", "extensions");
  const marker = join(cwd, "ambient_extension_loaded");
  mkdirSync(extensions, { recursive: true });
  writeFileSync(join(extensions, "canary.ts"), `import { writeFileSync } from "node:fs";
writeFileSync(${JSON.stringify(marker)}, "unexpected"); export default () => {};\n`);
  writeFileSync(join(cwd, "AGENTS.md"), "AMBIENT_CONTEXT_MUST_NOT_BE_LOADED\n");
  const child = spawn(process.execPath, ["--no-env-file", entry], {
    cwd, env: { HOME: cwd }, stdio: ["pipe", "pipe", "pipe"], timeout: 3000, killSignal: "SIGKILL",
  });
  child.stdin.on("error", () => { /* Expected if the child closes after an over-limit frame. */ });
  child.stderr.setEncoding("utf8");
  let stderr = "";
  child.stderr.on("data", (data) => { stderr += data; });
  const exit = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) => resolve(code));
  });
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  try {
    await check({
      raw: (bytes) => { child.stdin.write(bytes); },
      send: (frame) => { child.stdin.write(`${JSON.stringify(frame)}\n`); },
      receive: async () => {
        const next = await lines.next();
        if (next.done) throw new Error("unexpected closed fixture stdout");
        return JSON.parse(next.value);
      },
      expectExit: async (code) => {
        expect(await exit).toBe(code);
        expect((await lines.next()).done).toBe(true);
        expect(stderr).toBe("");
        expect(existsSync(marker)).toBe(false);
      },
    });
  } finally {
    child.kill("SIGKILL");
    await exit;
    rmSync(cwd, { recursive: true, force: true });
  }
}

const initializeRequest = {
  type: "mcp", request: {
    jsonrpc: "2.0", id: 1, method: "initialize", params: {
      protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "corral-synthetic", version: "1" },
    },
  },
};
const shimInfo = {
  protocolVersion: "2025-06-18", capabilities: { tools: {} },
  serverInfo: { name: "corral-lite-mcp-shim", version: "1" },
};
const initializeReply = { jsonrpc: "2.0", id: 1, result: shimInfo };
// Deliberately return the evidence descriptors in a different order than the provider registry.
const evidenceTools = start.tools.slice(0, 3).reverse();
const listReply = { jsonrpc: "2.0", id: 2, result: { tools: evidenceTools } };

async function expectList(pi: Pi) {
  expect(await pi.receive()).toEqual({ type: "mcp", request: { jsonrpc: "2.0", method: "notifications/initialized" } });
  // The notification receives no reply; Pi must issue tools/list on its own.
  expect(await pi.receive()).toEqual({ type: "mcp", request: { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} } });
}

async function negotiate(pi: Pi) {
  expect(await pi.receive()).toEqual(initializeRequest);
  pi.send({ type: "mcp", response: initializeReply });
  await expectList(pi);
  pi.send({ type: "mcp", response: listReply });
}

function call(id: string, name: string, args: string) {
  return { id, type: "function", function: { name, arguments: args } };
}

function assistant(...calls: ReturnType<typeof call>[]) {
  return { role: "assistant", content: null, tool_calls: calls };
}

const search = call("search-1", "search_materials", '{ "query": "February", "limit": 2 }');
const readSlice = call("read-1", "read_evidence", '{"source":"feb","start_byte":0,"max_bytes":200}');
const metric = call("metric-1", "calculate_metric", '{"metric":"subtract_expenses_v1","inputs":["feb"]}');
const finish = call("finish-1", "finish_report", '{"text":"Cash on hand is not established.","citations":[]}');

function dispatch(request: ReturnType<typeof call>) {
  return { type: "tool", id: request.id, name: request.function.name, args: JSON.parse(request.function.arguments) };
}

test("Pi starts via MCP, then executes the pinned four-tool Agent registry and projects exact C1 history", async () => {
  await withPi(async (pi) => {
    // Split the start frame in the middle of a UTF-8 code point.
    const raw = Buffer.from(`${JSON.stringify(start)}\n`);
    const offset = raw.indexOf(Buffer.from("’")) + 1;
    pi.raw(raw.subarray(0, offset));
    pi.raw(raw.subarray(offset));
    await negotiate(pi);
    expect(await pi.receive()).toEqual({ type: "turn", body: pinnedTurn });
    const messages: object[] = [...pinnedTurn.messages];
    const step = async (entries: [ReturnType<typeof call>, string][]) => {
      const message = assistant(...entries.map(([call]) => call));
      pi.send({ type: "provider", message });
      messages.push(message);
      for (const [call, content] of entries) {
        expect(await pi.receive()).toEqual(dispatch(call));
        pi.send({ type: "result", content });
        messages.push({ role: "tool", tool_call_id: call.id, content });
      }
    };
    await step([[search, '{"hits":[{"label":"February","source":"feb"}],"status":"ok"}']]);
    expect(await pi.receive()).toEqual({ type: "turn", body: { ...pinnedTurn, messages } });
    await step([
      [readSlice, '{"citation":"cite_inputs","source":"feb","source_version":"feb_1","start_byte":0,"status":"ok","text":"February."}'],
      // Supervisor supplied a terminal result for this original calculate_metric tool ID; no host poll tool is exposed.
      [metric, '{"cell":"feb_net","citation":"cite_net","limitations":"Not cash on hand.","operation_version":"demo_1","result":"res_feb","result_kind":"captured","status":"ok","unit":"units","value_minor":2300}'],
    ]);
    expect(await pi.receive()).toEqual({ type: "turn", body: { ...pinnedTurn, messages } });
    await step([[finish, '{"status":"ok"}']]);
    expect(await pi.receive()).toEqual({ type: "done" });
    await pi.expectExit(0); // Leave input open; a second start/retry/turn is not possible.
  });
});

test("changed prompt bytes or provider registry order are refused before MCP negotiation", async () => {
  for (const change of [{ prompt: start.prompt.trimEnd() }, { tools: start.tools.toReversed() }]) {
    await withPi(async (pi) => {
      pi.send({ ...start, ...change });
      await pi.expectExit(1);
    });
  }
});

test("invalid shim initialize replies stop Pi before notification, listing, or provider", async () => {
  for (const response of [
    { ...initializeReply, id: 2 },
    { ...initializeReply, jsonrpc: "1.0" },
    { ...initializeReply, result: { ...shimInfo, protocolVersion: "2024-11-05" } },
    { ...initializeReply, result: { ...shimInfo, serverInfo: { ...shimInfo.serverInfo, name: "mitimind-lite-analyst" } } },
    { ...initializeReply, result: { ...shimInfo, capabilities: { tools: { listChanged: false } } } },
  ]) {
    await withPi(async (pi) => {
      pi.send(start);
      expect(await pi.receive()).toEqual(initializeRequest);
      pi.send({ type: "mcp", response });
      await pi.expectExit(1);
    });
  }
});

test("invalid shim tools/list replies stop Pi without a provider turn", async () => {
  for (const response of [
    { ...listReply, id: 1 },
    { ...listReply, result: { ...listReply.result, nextCursor: "another-page" } },
    { ...listReply, result: { tools: [{ ...evidenceTools[0]!, description: "changed" }, ...evidenceTools.slice(1)] } },
    { ...listReply, result: { tools: [...evidenceTools, start.tools[3]!] } },
  ]) {
    await withPi(async (pi) => {
      pi.send(start);
      expect(await pi.receive()).toEqual(initializeRequest);
      pi.send({ type: "mcp", response: initializeReply });
      await expectList(pi);
      pi.send({ type: "mcp", response });
      await pi.expectExit(1);
    });
  }
});

test("forbidden tools and invalid args reject the entire provider batch without retries or fallback", async () => {
  for (const bad of [
    call("bad", "read_result", '{"result":"res_feb"}'),
    call("bad", "bash", '{"command":"touch unexpected"}'),
    call("bad", "search_materials", '{"query":"ok","limit":"2"}'), // Pi's SDK normally coerces this.
    call("bad", "search_materials", '{"query":"ok","limit":2,"li\\u006dit":3}'),
    call("bad", "read_evidence", '{"source":"feb","start_byte":0,"max_bytes":4097}'),
  ]) {
    await withPi(async (pi) => {
      pi.send(start);
      await negotiate(pi);
      expect(await pi.receive()).toEqual({ type: "turn", body: pinnedTurn });
      pi.send({ type: "provider", message: assistant(search, bad) });
      await pi.expectExit(1);
    });
  }
});

test("missing finish and an unacknowledged finish never trigger another provider turn", async () => {
  await withPi(async (pi) => {
    pi.send(start);
    await negotiate(pi);
    expect(await pi.receive()).toEqual({ type: "turn", body: pinnedTurn });
    pi.send({ type: "provider", message: { role: "assistant", content: "Not a sealed report." } });
    await pi.expectExit(1);
  });
  await withPi(async (pi) => {
    pi.send(start);
    await negotiate(pi);
    expect(await pi.receive()).toEqual({ type: "turn", body: pinnedTurn });
    pi.send({ type: "provider", message: assistant(finish) });
    expect(await pi.receive()).toEqual(dispatch(finish));
    pi.send({ type: "result", content: '{"status":"error"}' });
    await pi.expectExit(1);
  });
});

test("the LF IPC refuses over-limit frames without waiting for a terminator", async () => {
  await withPi(async (pi) => {
    pi.raw(Buffer.alloc(524_289, 0x20));
    await pi.expectExit(1);
  });
});
