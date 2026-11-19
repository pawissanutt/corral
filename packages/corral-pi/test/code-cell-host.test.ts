// End to end: a Code Mode cell in the rich transcript shows the calls it made, live, through a real HostSession,
// InteractiveMode and the native panels (CODE-1, CODE-2, CODE-3, CODE-4). Only the provider and control plane are
// fixtures; the kernel, the nested `read` calls and the renderers are the shipped ones. One nested read is held in a
// `tool_call` hook, so the running state is observed by barrier, never by timing.
import { expect, test } from "bun:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { fixtureTransport } from "./fixtures/auth/index.ts";
import { startPanelHost, type PanelHost } from "./support/panel-host.ts";
import { nextEvent, within } from "./support/wait.ts";

const CODE_MODE = { catalog: 10, digest: "a".repeat(64), preset: "standard", enabled: ["codemode"] };
const SCRIPT = "const settled = await Promise.allSettled([tools.read({ path: \"present.txt\" }), tools.read({ path: \"missing.md\" })]);"
  + " print(\"SCRIPT_PRINTED \" + settled.map(item => item.status).join(\",\"));";
const SPINNER = /[⠋⠙⠹⠸⠼⠴⠦⠧●]/;

const type = (h: PanelHost, text: string) => { for (const char of text) h.physical.send(char); };
const lines = (h: PanelHost) => h.screen().lines();
/** The cell's header row: the one naming `code` with its call count. */
const header = (h: PanelHost) => lines(h).find(line => /\bcode\b.*\b2 calls\b/.test(line));

test("a Code Mode cell shows the calls it made, live, and its final state", async () => {
  const { promise: reached, resolve: entered } = Promise.withResolvers<void>();
  const { promise: held, resolve: release } = Promise.withResolvers<void>();
  const h = await startPanelHost(pi => pi.on("tool_call", async event => {
    // Only the cell's nested read of the existing file waits; the missing file's read fails at once.
    if (event.parentToolCallId === undefined || event.toolName !== "read") return undefined;
    const path: unknown = Reflect.get(event.input, "path");
    if (path === "present.txt") { entered(); await held; }
    return undefined;
  }), { capabilities: CODE_MODE });
  try {
    writeFileSync(join(h.work, "present.txt"), "PRESENT_FILE_LINE\n");
    fixtureTransport().script([
      { toolCalls: [{ callId: "cell", name: "codemode", arguments: { code: SCRIPT } }] },
      { text: "CELL_REPLY_DONE" },
    ]);
    const ended = nextEvent(h.events, event => event.kind === "turn_ended");
    type(h, "Run the cell.");
    await h.physical.until("Run the cell.");
    const mark = h.physical.writes.length;
    h.physical.send("\r");

    // Live: one read held in flight, the other already failed.
    await within(reached, 10_000, "the nested read of present.txt");
    const row = (name: string) => lines(h).find(line => line.includes(name)) ?? "";
    await h.physical.whenWritten(() => (header(h) ?? "").includes("✗1") && /✗ .*missing\.md/.test(row("missing.md")),
      "the cell header counting the failed read");
    const live = header(h) ?? "";
    expect(live).toMatch(new RegExp(`${SPINNER.source} code`));
    expect(row("present.txt")).toMatch(new RegExp(`${SPINNER.source} .*present\\.txt`));
    expect(h.screen().text()).toMatch(/ENOENT|not found|no such file/i);

    // Settled: the script caught the failure, so the cell reads ok with the failure counted.
    release();
    await ended;
    await h.host.drained();
    await h.physical.untilVisible("CELL_REPLY_DONE", mark);
    await h.physical.whenWritten(() => /^\s*✓ code/.test(header(h) ?? ""), "the settled cell header");
    const final = header(h) ?? "";
    expect(final).toContain("✓1");
    expect(final).toContain("✗1");
    expect(row("present.txt")).toMatch(/✓ .*present\.txt/);
    expect(row("missing.md")).toMatch(/✗ .*missing\.md/);
    expect(h.screen().text()).toContain("SCRIPT_PRINTED fulfilled,rejected");

    // Collapsed, neither the script source nor the model-facing report is drawn.
    const screen = h.screen().text();
    for (const raw of ["Promise.allSettled", "tools.read(", "Cell cell-1: ok", "Nested calls:"]) {
      expect(screen).not.toContain(raw);
    }
  } finally {
    release();
    await h.stop();
  }
});
