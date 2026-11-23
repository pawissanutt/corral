import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { serializeCodeSeed, superviseCode, writeCodeSeed } from "../src/host/code-handoff.ts";

test("the supervisor waits for a successful old exit and hands seed and draft to a fresh process", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "corral-handoff-"));
  try {
    const fixture = join(cwd, "host.ts");
    const module = new URL("../src/host/code-handoff.ts", import.meta.url).href;
    writeFileSync(fixture, `import { receiveCodeBootstrap, sendCodeLaunch } from ${JSON.stringify(module)};
      import { writeFileSync } from "node:fs";
      const data = await receiveCodeBootstrap();
      if (process.argv[3] === "old") {
        await sendCodeLaunch({ cwd: process.cwd(), args: ["new"], seed: "copy", draft: "unsent" });
        await Bun.sleep(50);
        writeFileSync("old", String(process.pid));
        process.exit(Number(process.argv[4]));
      } else {
        const oldPid = Number(await Bun.file("old").text());
        try { process.kill(oldPid, 0); process.exit(3); } catch {}
        writeFileSync("new", JSON.stringify({ pid: process.pid, data, cwd: process.cwd() }));
      }`);
    const command = [process.execPath, fixture];
    expect(await superviseCode(command, { cwd, args: ["old", "2"] })).toBe(2);
    expect(existsSync(join(cwd, "new"))).toBe(false);
    expect(await superviseCode(command, { cwd, args: ["old", "0"] })).toBe(0);
    const { pid, ...next } = JSON.parse(readFileSync(join(cwd, "new"), "utf8"));
    expect(typeof pid).toBe("number");
    expect(next).toEqual({ cwd, data: { seed: "copy", draft: "unsent" } });
  } finally { rmSync(cwd, { recursive: true, force: true }); }
});

test("the copied transcript receives a private file and fresh Pi header without rewriting its branch", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-seed-"));
  try {
    const entry = { type: "session_info", name: "copied", id: "seed", parentId: null, timestamp: new Date().toISOString() } as const;
    const seed = serializeCodeSeed([entry]);
    const a = writeCodeSeed(dir, dir, 3, seed);
    const b = writeCodeSeed(dir, dir, 3, seed);
    const [header, copied] = readFileSync(a, "utf8").trimEnd().split("\n").map(line => JSON.parse(line));
    expect(a).not.toBe(b);
    const { id, timestamp, ...metadata } = header;
    expect(typeof id).toBe("string");
    expect(typeof timestamp).toBe("string");
    expect(metadata).toEqual({ type: "session", version: 3, cwd: dir });
    expect(copied).toEqual(entry);
    if (process.platform !== "win32") expect(statSync(a).mode & 0o777).toBe(0o600);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
