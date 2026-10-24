import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  CorralCallError, CorralClient, canonicalJson, eventRequestId, sha256Hex, toolRequestId,
} from "../src/tools/corral.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";

const dir = mkdtempSync(join(tmpdir(), "corral-pi-client-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

const HOST_CAP = "h".repeat(64);
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
