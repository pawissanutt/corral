import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  CorralCallError, CorralClient, artifactReference, canonicalJson, eventRequestId, sha256Hex, toolRequestId,
} from "../src/tools/corral.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";

const dir = mkdtempSync(join(tmpdir(), "corral-pi-client-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

const HOST_CAP = "h".repeat(64);

function scriptedClient(body: string, timeoutMs = 2000) {
  const work = mkdtempSync(join(dir, "script-"));
  const path = join(work, "corral");
  writeFileSync(path, `#!/usr/bin/env bun\nimport { readFileSync, writeFileSync } from "node:fs";\nconst input = JSON.parse(readFileSync(0, "utf8"));\n${body}`, { mode: 0o700 });
  return { work, client: new CorralClient({ corral: path, root: work, identity, hostCap: HOST_CAP,
    sessionId: () => "s-1", timeoutMs }) };
}

const mutation = { requestId: toolRequestId("s-1", "mutation"), payload: { task: "T-001", text: "effect" } };

async function failed(client: CorralClient): Promise<CorralCallError> {
  try { await client.call("comment", mutation); } catch (error) {
    expect(error instanceof CorralCallError).toBeTrue();
    return error as CorralCallError;
  }
  throw new Error("expected transport failure");
}
const NONCE = "n".repeat(64);
const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 2 };

test("canonical JSON and request IDs match envelope.rs", () => {
  const payload = { turn: 3, kind: "turn_started", a: [1, "x", { c: true, b: null }] };
  expect(canonicalJson(payload)).toBe('{"a":[1,"x",{"b":null,"c":true}],"kind":"turn_started","turn":3}');
  expect(sha256Hex(canonicalJson(payload))).toBe("ee3f28540ffe3d5555f1d0c02c2c45e59912380e7fd41061b776d59843c33f71");
  expect(toolRequestId("session-1", "call-1")).toBe("rq-eb8ccf651372cda868564a602768dbbd");
  expect(eventRequestId(2, 7)).toBe("ev-2-7");
  expect(() => canonicalJson({ ratio: 0.5 })).toThrow(/safe integers/);
});

test("a call is one fresh corral process whose secrets travel only on stdin", async () => {
  const corral = writeFakeCorral(dir);
  const client = new CorralClient({ corral: corral.path, root: dir, identity, hostCap: HOST_CAP, sessionId: () => "s-1" });
  const receipt = await client.call("hello", { requestId: "ev-2-0", payload: { pid: 7 }, bindNonce: NONCE });
  expect(receipt.state).toBe("committed");
  const [call] = corral.calls();
  expect(call?.argv).toEqual(["agent", "call", "--op", "hello"]);
  expect(call?.argv.join(" ")).not.toContain(HOST_CAP);
  expect(call?.envelope.auth).toEqual({ host_cap: HOST_CAP, bind_nonce: NONCE });
  expect(call?.envelope.payload_sha256).toBe(sha256Hex(canonicalJson({ pid: 7 })));
  expect(call?.envelope.identity).toEqual({ ...identity, session_id: "s-1" });
});

test("a failed or malformed corral call is an error, never a receipt", async () => {
  const failing = mkdtempSync(join(dir, "fail-"));
  const corral = writeFakeCorral(failing);
  corral.failWith(3);
  const client = new CorralClient({ corral: corral.path, root: failing, identity, hostCap: HOST_CAP, sessionId: () => "s-1" });
  let error: unknown;
  try {
    await client.call("event", { requestId: "ev-2-1", payload: { kind: "turn_started", turn: 1 } });
  } catch (caught) {
    error = caught;
  }
  expect(error instanceof CorralCallError).toBeTrue();
  expect((error as CorralCallError).code).toBe("call_failed");
  expect(String(error)).not.toContain(HOST_CAP);
});

test("nonzero exit and signal preserve bounded diagnostics, redaction and unknown effect", async () => {
  for (const [body, exit_code, signal] of [
    ['process.stderr.write("detail " + input.auth.host_cap); process.exit(23);', 23, null],
    ['process.stderr.write("signal detail"); process.kill(process.pid, "SIGTERM");', null, "SIGTERM"],
  ] as const) {
    const { client } = scriptedClient(body);
    const error = await failed(client);
    expect(error.code).toBe("call_failed");
    expect(error.diagnostics).toMatchObject({ exit_code, signal, timed_out: false, effect: "unknown" });
    expect(error.diagnostics.diagnostic).toContain("detail");
    expect(error.diagnostics.diagnostic!.length).toBeLessThanOrEqual(4096);
    expect(String(error)).not.toContain(HOST_CAP);
    expect(error.diagnostics.diagnostic).not.toContain(HOST_CAP);
  }
});

test("deadline kills a stopped executor after its effect, without claiming not-started", async () => {
  const { client, work } = scriptedClient('writeFileSync("effect", "committed before ack"); process.kill(process.pid, "SIGSTOP");');
  const error = await failed(client);
  expect(existsSync(join(work, "effect"))).toBeTrue();
  expect(error.code).toBe("call_timeout");
  expect(error.diagnostics).toMatchObject({ exit_code: null, signal: "SIGKILL", timed_out: true, effect: "unknown" });
});

test("a canonical receipt proves the effect even when the executor subsequently exits nonzero", async () => {
  const { client } = scriptedClient(`process.stdout.write(JSON.stringify({version:1, request_id:input.request_id,
    op:input.op, state:"committed", result:{effect:"recorded"}}) + "\\n"); process.exit(7);`);
  const receipt = await client.call("comment", mutation);
  expect(receipt.state).toBe("committed");
  expect(receipt.request_id).toBe(mutation.requestId);
  expect(receipt.result).toEqual({ effect: "recorded" });
});

test("malformed or foreign receipt and bounded output failure retain uncertain effect", async () => {
  for (const body of [
    'process.stdout.write("not JSON");',
    'process.stdout.write(JSON.stringify({version:1, request_id:"foreign", op:input.op, state:"committed", result:{}}));',
    'process.stderr.write("x".repeat(300000));',
  ]) {
    const { client } = scriptedClient(body);
    const error = await failed(client);
    expect(error.code).toBe("call_output");
    expect(error.diagnostics.effect).toBe("unknown");
    expect(error.diagnostics.diagnostic!.length).toBeLessThanOrEqual(4096);
  }
});

test("receipt lookup returns the original receipt plus canonical lookup identity", async () => {
  const { client } = scriptedClient(`process.stdout.write(JSON.stringify({version:1, request_id:input.payload.request_id,
    op:"comment", state:"committed", result:{effect:"recorded"},
    lookup:{by:input.request_id, request_id:input.payload.request_id, found:true}}));`);
  const receipt = await client.call("receipt_lookup", { requestId: toolRequestId("s-1", "lookup"),
    payload: { request_id: mutation.requestId, task: "T-001" } });
  expect(receipt.request_id).toBe(mutation.requestId);
  expect(receipt.lookup).toMatchObject({ found: true });
});

test("artifact viewers accept canonical stored names as one argv element and refuse malformed references", async () => {
  const work = mkdtempSync(join(dir, "viewer-"));
  const fake = writeFakeCorral(work);
  writeFileSync(join(work, "artifact-show.txt"), "shown\n");
  const client = new CorralClient({ corral: fake.path, root: work, identity, hostCap: HOST_CAP, sessionId: () => "s-1" });
  const names = ["T-001:R1:12345678-my report.md", "T-001:R1:12345678-résumé.png", "T-001:R1:12345678-a:b $(touch x).md", "A-7:12345678-notes 📝.md"];
  for (const name of names) {
    expect(await client.artifactViewer(name, "show")).toBe("shown\n");
    expect(await client.artifactViewer(name, "serve")).toBe("http://127.0.0.1:9/fixture\n");
  }
  const argv = readFileSync(join(work, "viewer-calls.jsonl"), "utf8").trim().split("\n").map(line => JSON.parse(line) as string[]);
  expect(argv).toEqual(names.flatMap(name => [["--root", work, "artifact", "show", name], ["--root", work, "artifact", "serve", name]]));
  expect(existsSync(join(work, "x"))).toBeFalse();
  for (const bad of ["T-001:R1:", "T-001:R1: padded.md", "T-001:R1:a/b.md", "T-001:R1:a\\b.md", "T-001:R1:a\nb.md", "--root", "T-001", "A-7:a:b.md", "X-1:R1:a.md"]) {
    expect(artifactReference(bad)).toBeFalse();
    await expect(client.artifactViewer(bad, "show")).rejects.toThrow("Invalid artifact reference");
  }
  expect(readFileSync(join(work, "viewer-calls.jsonl"), "utf8").trim().split("\n").length).toBe(names.length * 2);
});
