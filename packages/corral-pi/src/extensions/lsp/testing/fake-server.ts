// A scripted language server for the LSP extension's tests, run by argv as
// `bun fake-server.ts [--exit-on-definition] [--oversize-definition]`. It derives every answer
// from the opened file's text: each line containing ERR is an error diagnostic, `def` and `use`
// lines are a definition and its references. After every sync it opens a progress span and asks for
// configuration; the client's answer closes the span, publishes diagnostics and is echoed in them.
import { FrameDecoder, encodeFrame } from "../framing.ts";

const flags = new Set(process.argv.slice(2));
const documents = new Map<string, string>();
let serverRequest = 1000;
let configuration: unknown = "unrequested";
const awaitingConfiguration = new Map<number, { uri: string; token: string }>();

function send(message: Record<string, unknown>): void {
  process.stdout.write(encodeFrame({ jsonrpc: "2.0", ...message }));
}

const range = (line: number, start: number, end: number) => ({
  start: { line, character: start }, end: { line, character: end },
});

function lines(uri: string): string[] {
  return (documents.get(uri) ?? "").split("\n");
}

function publish(uri: string): void {
  const diagnostics = lines(uri).flatMap((text, line) => {
    const at = text.indexOf("ERR");
    return at < 0 ? [] : [{ range: range(line, at, at + 3), severity: 1, code: "E0001", source: "fake", message: text.trim() }];
  });
  if (flags.has("--report-cwd")) {
    diagnostics.push({ range: range(0, 0, 0), severity: 4, code: "cwd", source: "fake", message: process.cwd() });
  }
  if (configuration !== "unrequested") {
    diagnostics.push({ range: range(0, 0, 0), severity: 4, code: "config", source: "fake", message: JSON.stringify(configuration) });
  }
  send({ method: "textDocument/publishDiagnostics", params: { uri, diagnostics } });
}

function synced(uri: string): void {
  const token = `sync-${serverRequest}`;
  send({ id: serverRequest++, method: "window/workDoneProgress/create", params: { token } });
  send({ method: "$/progress", params: { token, value: { kind: "begin", title: "indexing" } } });
  const id = serverRequest++;
  awaitingConfiguration.set(id, { uri, token });
  send({ id, method: "workspace/configuration", params: { items: [{ section: "fake" }, { section: "other" }] } });
}

function configured(id: number, result: unknown): void {
  const sync = awaitingConfiguration.get(id);
  if (!sync) return;
  awaitingConfiguration.delete(id);
  configuration = result;
  publish(sync.uri);
  send({ method: "$/progress", params: { token: sync.token, value: { kind: "end" } } });
}

function locations(uri: string, word: string) {
  return lines(uri).flatMap((text, line) => {
    const at = text.indexOf(word);
    return at < 0 ? [] : [{ uri, range: range(line, at, at + word.length) }];
  });
}

function handle(message: Record<string, unknown>): void {
  const params = message.params as Record<string, unknown> | undefined;
  const doc = params?.textDocument as { uri: string; text?: string } | undefined;
  switch (message.method) {
    case "initialize":
      send({ id: message.id, result: { capabilities: { definitionProvider: true, referencesProvider: true, textDocumentSync: 1 } } });
      return;
    case "textDocument/didOpen":
      if (doc) documents.set(doc.uri, doc.text ?? "");
      if (doc) synced(doc.uri);
      return;
    case "textDocument/didChange": {
      const changes = params?.contentChanges as { text: string }[] | undefined;
      if (doc && changes?.[0]) documents.set(doc.uri, changes[0].text);
      if (doc) synced(doc.uri);
      return;
    }
    case "textDocument/definition":
      if (flags.has("--exit-on-definition")) process.exit(3);
      if (flags.has("--oversize-definition")) {
        send({ id: message.id, result: { uri: doc?.uri, range: range(0, 0, 0), padding: "x".repeat(64 * 1024) } });
        return;
      }
      send({ id: message.id, result: doc ? locations(doc.uri, "def ").slice(0, 1) : null });
      return;
    case "textDocument/references": {
      const context = params?.context as { includeDeclaration?: boolean } | undefined;
      const found = doc ? locations(doc.uri, "use ") : [];
      send({ id: message.id, result: context?.includeDeclaration && doc ? [...locations(doc.uri, "def "), ...found] : found });
      return;
    }
    case "shutdown":
      send({ id: message.id, result: null });
      return;
    case "exit":
      process.exit(0);
      return;
    default:
      if (message.method === undefined && typeof message.id === "number") {
        configured(message.id, message.result);
      } else if (message.method !== undefined && message.id !== undefined) {
        send({ id: message.id, error: { code: -32601, message: "unsupported" } });
      }
  }
}

const decoder = new FrameDecoder(1024 * 1024);
process.stdin.on("data", (chunk: Buffer) => {
  for (const message of decoder.push(chunk)) handle(message as Record<string, unknown>);
});
