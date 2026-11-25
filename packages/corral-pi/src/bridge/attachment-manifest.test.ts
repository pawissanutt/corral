import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { chmodSync, linkSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connect, testHandlers } from "../../test/support/bridge-client.ts";
import { parseManifestSubmitInput } from "./attachment-manifest.ts";
import { EventLog } from "./events.ts";
import { FRAME_MAX_BYTES, type SessionInput } from "./protocol.ts";
import { startBridge } from "./server.ts";

const name = `attachments-${"M".repeat(24)}.json`;
const digest = (data: string) => createHash("sha256").update(data).digest("hex");
const link = { id: "a1", kind: "link", name: "Link", url: "https://example.com/" };

function stage(dir: string, attachments: unknown) {
  const data = JSON.stringify(attachments);
  writeFileSync(join(dir, name), data, { mode: 0o600 });
  return { name, bytes: Buffer.byteLength(data), sha256: digest(data) };
}

test("bounded submit_input_manifest hydrates large metadata before ordinary owner admission", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-manifest-"));
  try {
    const sessionDir = join(dir, "session");
    mkdirSync(sessionDir, { mode: 0o700 });
    const attachments = Array.from({ length: 180 }, (_, index) => ({ id: `a${index + 1}`, kind: "file",
      name: "n".repeat(160), media_type: "t".repeat(160), bytes: 1, sha256: digest("x") }));
    const manifest = stage(sessionDir, attachments);
    expect(manifest.bytes).toBeGreaterThan(FRAME_MAX_BYTES);
    const submitted: SessionInput[] = [];
    const server = await startBridge({ path: join(dir, "host.sock"), sessionId: "fixture", sessionDir, events: new EventLog(),
      handlers: testHandlers({ submit: async input => { submitted.push(input); return { disposition: "queued" }; } }) });
    try {
      const client = await connect(server.path);
      try {
        const request = { version: 2, id: "req", command: "submit_input_manifest", args: { origin: "owner", text: "",
          input_id: "phone:1", to: "parent", typed_at: "2026-11-18T12:00:00Z", device: "phone", busy_policy: "follow_up",
          attachments_manifest: manifest } };
        for (const command of ["submit_input", "submit_automation_v2"]) {
          expect((await client.request({ ...request, command })).error?.code).toBe("bad_attachment");
        }
        expect(submitted).toEqual([]);
        expect((await client.request(request)).result).toEqual({ request_id: "req", input: "phone:1", disposition: "queued" });
        expect(submitted).toEqual([{ text: "", origin: "owner", busyPolicy: "follow_up", inputId: "phone:1",
          to: "parent", typedAt: "2026-11-18T12:00:00Z", device: "phone", attachments }]);
      } finally { client.close(); }
    } finally { await server.close(); }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("manifest reads reject nonowner input, paths, link escapes, tampering, and unvalidated metadata", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-manifest-"));
  try {
    const session = join(dir, "first");
    const other = join(dir, "other");
    for (const path of [session, other]) mkdirSync(path, { mode: 0o700 });
    const manifest = stage(other, [link]);
    const args = { origin: "owner", text: "", attachments_manifest: manifest };
    expect(() => parseManifestSubmitInput(args, session)).toThrow("bad_attachment");
    symlinkSync(join(other, name), join(session, name));
    expect(() => parseManifestSubmitInput(args, session)).toThrow("bad_attachment");
    rmSync(join(session, name));
    linkSync(join(other, name), join(session, name));
    expect(() => parseManifestSubmitInput(args, session)).toThrow("bad_attachment");
    rmSync(join(session, name));
    stage(session, [link]);
    for (const input of [
      { ...args, origin: "auto" },
      { ...args, attachments: [] },
      { ...args, attachments_manifest: { ...manifest, name: `../other/${name}` } },
      { ...args, attachments_manifest: { ...manifest, bytes: manifest.bytes + 1 } },
      { ...args, attachments_manifest: { ...manifest, bytes: 1024 * 1024 + 1 } },
      { ...args, attachments_manifest: { ...manifest, sha256: digest("wrong") } },
    ]) expect(() => parseManifestSubmitInput(input, session)).toThrow("bad_attachment");
    expect(parseManifestSubmitInput(args, session).attachments).toEqual([link]);
    chmodSync(join(session, name), 0o644);
    expect(() => parseManifestSubmitInput(args, session)).toThrow("bad_attachment");
    chmodSync(join(session, name), 0o600);
    writeFileSync(join(session, name), JSON.stringify([{ ...link, id: "a2" }]));
    expect(() => parseManifestSubmitInput(args, session)).toThrow("bad_attachment");
    const invalid = stage(session, [{ ...link, path: "/tmp/file" }]);
    expect(() => parseManifestSubmitInput({ ...args, attachments_manifest: invalid }, session)).toThrow("invalid attachment metadata keys");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
