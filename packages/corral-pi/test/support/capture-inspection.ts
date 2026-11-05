// Read-only real-input captures of production components. No host launch, model, receipts or pane.
// bun test/support/capture-inspection.ts <corral> <workspace> <task> <run> <output>
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { Text, VStack, stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { CorralClient } from "../../src/tools/corral.ts";
import { NativePanels } from "../../src/ui/panels.ts";
import { pad, PANEL_MUTED, PANEL_BG, RESET } from "../../src/ui/panels-view.ts";
import { RecordingTerminal } from "./panel-terminal.ts";

const [binary, workspace, task, run, output] = process.argv.slice(2);
if (!binary || !workspace || !task || !run || !output) throw new Error("corral, workspace, task, run, output required");
const out = resolve(output); mkdirSync(out, { recursive: true });
const identity = { store: "read-only-render-capture", project: "corral", role: "worker" as const, task, run, generation: 0 };
const client = new CorralClient({ corral: resolve(binary), root: resolve(workspace), identity, hostCap: "", sessionId: () => "no-live-session" });
let reads = 0;
const panels = new NativePanels({ identity, cwd: workspace, snapshot: () => null, inspection: () => { reads++; return client.inspectionSnapshot(); } });
const terminal = new RecordingTerminal();
const adapter = panels.wrap(terminal); adapter.start(() => {}, () => {}); adapter.write("\x1b[?1049h");
const main = panels.mainView(new Text("No conversation attached", 0, 0));
const root = new VStack([
  { component: main, basis: 0, grow: 1, minSize: 1 },
  { component: new Text("Read-only render capture · no host, editor or provider attached", 0, 0), basis: 2, shrink: 0 },
]);
const results: Record<string, unknown> = { source: { task, run }, runtime: "production native components, isolated in-memory terminal; not a bound host", frames: [] };
function frame(name: string, columns: number, rows: number) {
  terminal.resize(columns, rows);
  const width = adapter.columns;
  const layout = renderLayoutFrame(root, width, rows - 2, () => {});
  const rail = panels.wide ? panels.renderSidebar(34, rows - 2) : [];
  const lines = [PANEL_BG + PANEL_MUTED + pad(`READ-ONLY QA · canonical ${task}/${run} · no live generation`, columns) + RESET,
    ...layout.lines.map((line, i) => pad(line, width) + (panels.wide ? "│" + pad(rail[i] ?? "", 34) : ""))];
  for (const line of lines) if (visibleWidth(line) > columns) throw new Error("Capture overflow");
  writeFileSync(join(out, name + ".ansi"), lines.join("\n") + "\n");
  writeFileSync(join(out, name + ".txt"), stripTerminalSequences(lines.join("\n")) + "\n");
  (results.frames as string[]).push(name);
  return layout;
}
try {
  panels.handleInput("\x1b[18~"); await panels.loadFiles();
  if (panels.workbench.review.error) throw new Error(panels.workbench.review.error);
  const inspection = panels.workbench.inspection.current;
  if (!inspection) throw new Error("Canonical inspection unavailable");
  Object.assign(results, { base: inspection.base, head: inspection.head, manifest: inspection.manifest_digest, files: panels.workbench.review.files.length });
  writeFileSync(join(out, "canonical-inspection.json"), JSON.stringify(inspection, null, 2));
  if (panels.workbench.review.files.length < 2) throw new Error("Capture needs at least two real changed paths to prove selection");
  const firstSelected = panels.workbench.review.selected;
  frame("changes-wide", 140, 46);
  await panels.workbench.selectRelative(1);
  if (panels.workbench.review.selected === firstSelected) throw new Error("Keyboard did not change the selected file");
  frame("changes-keyboard", 140, 46);
  frame("changes-narrow", 80, 34);
  panels.handleInput("\x1b[21~"); await panels.workbench.inspection.refresh();
  let layout = frame("evidence-wide", 140, 46);
  // Click the real Evidence nav hit region; the existing same-instance transcript is retained.
  const nav: TuiMouseEvent = { type: "click", button: "left", x: 45, y: 0, screenX: 45, screenY: 0, width: layout.width, height: layout.height, shift: false, alt: false, ctrl: false };
  for (const box of getLayoutBoxesAt(layout, nav.screenX, nav.screenY)) {
    if (box.component.handleMouse?.({ ...nav, x: nav.screenX - box.rect.x, y: nav.screenY - box.rect.y })) break;
  }
  if (panels.workbench.view !== "evidence") throw new Error("Evidence pointer navigation failed");
  frame("evidence-narrow", 80, 34);
  panels.workbench.handleInput("\x1b[6~"); frame("evidence-narrow-details", 80, 34);
  Object.assign(results, { status: "PASS", canonical_reads: reads, checks: ["canonical run inspection", "real committed file list and hunks", "keyboard file selection", "Evidence keyboard/pointer navigation", "140x46 and 80x34 width bounds"] });
  writeFileSync(join(out, "result.json"), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} finally { adapter.write("\x1b[?1049l"); adapter.stop(); panels.dispose(); }
