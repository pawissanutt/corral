// Child process for the socket tripwire test: the tripwire patches Node's modules for the whole
// process, so it is armed here rather than in the test runner. Every TCP target is loopback port 9
// or an .invalid host, so even a broken tripwire cannot reach a real network.
import { mkdtempSync } from "node:fs";
import http from "node:http";
import http2 from "node:http2";
import https from "node:https";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import tls from "node:tls";

import { hostRules } from "../../../src/auth/providers.ts";
import { guardedFetch } from "../../../src/host/restrict.ts";
import { installSocketTripwire } from "../../../src/host/smithy.ts";

const guard = guardedFetch(async () => new Response("unexpected"), undefined, "amazon-bedrock",
  hostRules("amazon-bedrock", { region: "us-west-2", model: "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
    assumesRole: false, ssoRegions: [] }));
installSocketTripwire((destination) => guard.refuse!(destination));

const results: Record<string, string> = {};
const attempt = (name: string, open: () => unknown) => {
  try {
    open();
    results[name] = "opened";
  } catch (error) {
    results[name] = (error as { code?: string }).code ?? String(error);
  }
};
attempt("https.request", () => https.request("https://bedrock-runtime.us-west-2.amazonaws.invalid/model/x/converse-stream?q=1"));
attempt("https.get", () => https.get({ host: "127.0.0.1", port: 9, path: "/" }));
attempt("http.request", () => http.request({ host: "127.0.0.1", port: 9, path: "/latest/api/token" }));
attempt("http2.connect", () => http2.connect("https://bedrock-runtime.us-west-2.amazonaws.invalid"));
attempt("tls.connect", () => tls.connect(9, "127.0.0.1"));
attempt("net.connect", () => net.connect(9, "127.0.0.1"));
attempt("net.createConnection", () => net.createConnection({ host: "127.0.0.1", port: 9 }));
attempt("Socket.connect", () => new net.Socket().connect(9, "127.0.0.1"));

const path = join(mkdtempSync(join(tmpdir(), "corral-tripwire-")), "s.sock");
const server = net.createServer((socket) => socket.end("pong"));
await new Promise<void>((resolve) => server.listen(path, resolve));
results.unix = await new Promise<string>((resolve, reject) => {
  const client = net.connect(path);
  let data = "";
  client.on("data", (chunk) => { data += chunk.toString(); });
  client.on("end", () => resolve(data));
  client.on("error", reject);
});
server.close();
process.stdout.write(`${JSON.stringify({ results, ledger: guard.ledger() })}\n`);
process.exit(0);
