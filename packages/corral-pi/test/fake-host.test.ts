import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { connect } from "./support/bridge-client.ts";
import { startFakeHost, type FakeHostScript } from "./support/fake-host.ts";

test("the scripted host answers hello, live_since with state, and a registered part as the daemon reads them", async () => {
  const root = mkdtempSync("/tmp/corral-fake-");
  try {
    const recording = SessionManager.create(root, root);
    recording.appendMessage({ role: "user", content: "Inspect the staged project.", timestamp: Date.now() });
    const original = recording.getSessionFile();
    if (!original) throw new Error("Pi did not persist the recording");
    const jobs = { jobs: [{ id: "j1-1", command: "sleep 30", state: "running", output_bytes: 0, tail: [], monitors: [], actions: ["stop"] }] };
    const script: FakeHostScript = { transcript: basename(original), state: { turn: 3, open: true, jobs: { live: 1, waiting: 0, total: 1 } },
      events: [{ kind: "ready" }], live: [{ kind: "live_part", data: { part: "jobs" } }], methods: { jobs } };
    const file = join(root, "script.json"), dir = join(root, "A-1"), socket = join(root, "host.sock");
    writeFileSync(file, JSON.stringify(script));
    const bridge = await startFakeHost(file, ["chat", "--dir", dir, "--headless", "--socket", socket], {}, root);
    try {
      // The engine's client opens a separate connection for each command.
      const read = async (command: string, args = {}) => {
        const client = await connect(socket);
        try { return await client.request({ version: 2, id: "daemon-1", command, args }); }
        finally { client.close(); }
      };
      const hello = await read("hello");
      expect(hello).toMatchObject({ version: 2, id: "daemon-1", ok: true,
        result: { protocol: 2, role: "assistant", session_id: "A-1", pid: process.pid } });
      const transcript = hello.result?.session_file;
      if (typeof transcript !== "string") throw new Error("hello did not name the Pi recording");
      expect(JSON.parse(readFileSync(join(dir, "session.json"), "utf8"))).toMatchObject({ socket, pid: process.pid, sessionFile: transcript });
      expect(transcript).toBe(join(dir, "sessions", "recorded.jsonl"));
      expect(readFileSync(transcript)).toEqual(readFileSync(original));
      expect(await read("events_since", { seq: 0 })).toMatchObject({ ok: true,
        result: { events: [{ seq: 1, kind: "ready" }], last: 1, next: 1, more: false } });
      expect(await read("live_since", { seq: 0 })).toMatchObject({ ok: true,
        result: { events: [{ seq: 1, kind: "live_part", part: "jobs" }, { seq: 2, kind: "live_state" }],
          next: 2, last: 2, more: false, state: { mode: "headless", turn: 3, open: true, jobs: { live: 1, total: 1 } } } });
      expect((await read("live_since", { seq: 2 })).result).toEqual({ events: [], next: 2, last: 2, more: false });
      expect(await read("jobs")).toMatchObject({ version: 2, id: "daemon-1", ok: true, result: jobs });
      expect(await read("usage")).toMatchObject({ ok: false, error: { code: "unknown_command" } });
    } finally { await bridge.close(); }
  } finally { rmSync(root, { recursive: true, force: true }); }
});
