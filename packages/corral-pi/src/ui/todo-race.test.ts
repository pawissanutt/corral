import { expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { stripTerminalSequences } from "@earendil-works/pi-tui";
import { TodoLedger } from "./todo.ts";
import { NativePanels } from "./panels.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

const identity = { store: "st-test", project: "demo", role: "worker" as const, task: "T-689", run: "R1", generation: 1 };
const canonical = { runs: [{ id: "R1", progress: { revision: 1, spec_revision: "spec", phases: [{ id: "p", label: "Work", steps: [
  { id: "s", label: "Await canonical reconciliation", status: { state: "in_progress" }, criteria: ["C1"] },
] }] } }] };
function barrier<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

for (const [name, outcome] of [
  ["uncertain", { state: "uncertain" }],
  ["reserved", { state: "reserved" }],
  ["queued", { state: "queued" }],
  ["unknown-effect", { error: { effect: "unknown" } }],
] as const) {
  test(`${name}: an older canonical read cannot erase newer mutation uncertainty`, async () => {
    let load = async (): Promise<unknown> => canonical;
    const ledger = new TodoLedger(identity, () => load());
    await ledger.restore();
    expect(ledger.unavailable).toBeFalse();
    const entered = barrier<void>(), blocked = barrier<unknown>();
    load = () => { entered.resolve(); return blocked.promise; };
    const olderRead = ledger.restore();
    await entered.promise; // The snapshot read is in flight before the uncertain receipt arrives.
    await ledger.receipt(outcome);
    expect(ledger.unavailable).toBeTrue();

    const snapshot = (): PanelSnapshot => ({ bound: true, model: null, effort: "off", streaming: false,
      context: undefined, tokens: 0, tools: [], contextFiles: [], skills: [], ledger: ledger.current,
      ledgerStale: ledger.unavailable, jobs: null, auxiliary: null, steering: 0, followUps: 0 });
    const panels = new NativePanels({ identity, cwd: ".", snapshot });
    const terminal = new RecordingTerminal();
    panels.wrap(terminal).start(() => {}, () => {});
    panels.sidebar.tab = "plan";
    const captured: string[] = [];
    const render = (label: string) => {
      terminal.resize(140, 24);
      const wide = stripTerminalSequences(panels.renderSidebar(80, 24).join("\n"));
      captured.push(label, "WIDE", wide);
      return { wide };
    };
    try {
      expect(render("Uncertain receipt while earlier read is blocked").wide).toContain("stale");
      blocked.resolve(canonical);
      expect(await olderRead).toBeFalse();
      expect(ledger.current?.revision).toBe(1);
      expect(ledger.unavailable).toBeTrue();
      const retained = render("Earlier read completed; uncertainty must remain visible");
      expect(retained.wide).toContain("stale");

      // A new read ordered after the uncertainty can establish freshness, even at the same revision.
      load = async () => canonical;
      expect(await ledger.restore()).toBeTrue();
      expect(ledger.unavailable).toBeFalse();
      const reconciled = render("Later canonical read completed; freshness established");
      expect(reconciled.wide).not.toContain("stale");
      const evidence = resolve(import.meta.dirname, "../../../../.evidence");
      mkdirSync(evidence, { recursive: true });
      writeFileSync(resolve(evidence, `T-689-race-${name}-render.txt`), captured.join("\n"));
    } finally {
      blocked.resolve(canonical);
      await olderRead;
      panels.dispose();
    }
  });
}
