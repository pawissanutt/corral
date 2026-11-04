import { expect, test } from "bun:test";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { agentRows } from "./agent-view.ts";
import { activityLines } from "./activity-view.ts";
import { NativeWorkbench } from "./workbench.ts";
import type { PanelSnapshot } from "./panels-view.ts";
const identity = { store: "s", project: "p", role: "worker" as const, task: "T-001", run: "R2", generation: 3 };
function snapshot(): PanelSnapshot {
  return { bound: true, model: "fixture", effort: "off", streaming: false, context: undefined, tokens: 5,
    tools: [], contextFiles: [], skills: [], ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null, jobs: [],
    agents: [{ id: "root", session: "root", kind: "worker", state: "settled locally", model: "fixture", transcript: true, tokens: 5 },
      { id: "child", session: "child", parent: "root", kind: "native explore", state: "uncertain", model: "fixture", transcript: true }] };
}
test("scoped identities, missing/stale observations and parent provenance never fabricate health/usage", () => {
  const rows = agentRows(identity, snapshot().agents, false, "child").map(row => row.text).join("\n");
  for (const text of ["Task T-001 / run R2 / assistant unavailable", "STALE", "NOT dependency", "Parent root", "Usage unavailable", "Health unavailable", "Transcript ineligible"]) expect(rows).toContain(text);
  expect(agentRows(identity, undefined, true).map(row => row.text).join("\n")).toContain("legacy/missing source");
  expect(agentRows(identity, [], true, "old").map(row => row.text).join("\n")).toContain("Selected agent missing");
  expect(agentRows({ ...identity, assistant: "A-9" }, [], true).map(row => row.text).join("\n")).toContain("assistant A-9");
});
test("completed/lost job observations and missing/stale output are explicit, not readiness/settlement", () => {
  const data = snapshot();
  const jobs = [{ job: "finished", state: "exited" }, { job: "gone", state: "lost" }];
  expect(activityLines({ ...data, jobs }, 1).join("\n")).toContain("? show legend");
  const lines = activityLines({ ...data, jobs }, 1, Date.now(), true).join("\n");
  expect(lines).toContain("1 exited · 1 lost"); expect(lines).toContain("Readiness unavailable");
  expect(lines).toContain("Detailed output unavailable"); expect(lines).toContain("No stop/restart");
  expect(activityLines({ ...data, jobs }, 10).join("\n")).toContain("Selected job missing");
  expect(activityLines({ ...data, jobs, bound: false }, 0).join("\n")).toContain("Output unavailable: stale/disconnected");
});
test("narrow main inspector input selects by stable identity, gates transcript reads, returns to unchanged chat", async () => {
  let current = snapshot(), reads = 0, overlays = 0;
  const notices: string[] = [];
  const wb = new NativeWorkbench({ cwd: "/tmp", identity, snapshot: () => current, changed() {}, canInteract: () => true,
    readTranscript: async id => { reads++; expect(id).toBe("child"); return "EXISTING_TRANSCRIPT"; } });
  const ui = { notify: (text: string) => notices.push(text), custom: async () => { overlays++; }, setEditorText: () => { throw new Error("must not write editor"); } } as unknown as ExtensionUIContext;
  wb.bind(ui);
  const body = wb.mount(new Text("ORIGINAL_CHAT", 0, 0));
  const screen = () => renderLayoutFrame(body, 40, 18, () => {}).lines;
  try {
    wb.handleInput("\x1b[19~"); // F8 jobs
    expect(wb.view).toBe("activity"); wb.handleInput("a"); expect(wb.view).toBe("agents");
    expect(screen().map(stripTerminalSequences).join("\n")).toContain("Scoped agents");
    expect(screen().every(line => visibleWidth(line) <= 40)).toBeTrue();
    wb.handleInput("j"); await wb.openAgentTranscript(); expect(reads).toBe(1); expect(overlays).toBe(1);
    current = { ...current, agents: current.agents?.slice(0, 1) }; // Selected child disappears, never silently selects parent.
    screen(); await wb.openAgentTranscript(); expect(reads).toBe(1); expect(notices.join()).toContain("missing");
    current = { ...snapshot(), bound: false }; await wb.openAgentTranscript(); expect(reads).toBe(1);
    wb.handleInput("b"); expect(wb.view).toBe("activity"); wb.handleInput("\x1b[20~");
    expect(wb.view).toBe("conversation"); expect(screen().map(stripTerminalSequences).join("\n")).toContain("ORIGINAL_CHAT");
  } finally { wb.dispose(); }
});
