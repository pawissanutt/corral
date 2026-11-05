// Real SDK -> Code Mode kernel -> authenticated tool -> isolated Corral executable -> UI.
import { expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { stripTerminalSequences } from "@earendil-works/pi-tui";
import { codeHost } from "./support/code-host.ts";
import { activityLines } from "../src/ui/activity-view.ts";
import type { PanelSnapshot } from "../src/ui/panels-view.ts";

const progress = (revision: number) => ({ revision, spec_revision: "spec", phases: [{ id: "p", label: "Implementation", steps: [
  { id: "s", label: `Work revision ${revision}`, status: { state: "in_progress" }, criteria: ["C1"] },
] }] });
const update = (revision: number) => ({ task: "T-683", run: "R1", spec_revision: "spec", expected_revision: revision - 1,
  phases: progress(revision).phases });

test("authenticated Code Mode children expose all receipt states and reconcile the rendered canonical ledger", async () => {
  const fx = await codeHost("worker", { enabled: ["codemode", "todo"], ui: "rich" });
  const panels = fx.host.panels;
  if (!panels) throw new Error("rich fixture must provide native panels");
  const captured: string[] = [];
  // Receipts render in the Activity view; the plan renders in the side panel.
  const snapshot = () => (panels as unknown as { options: { snapshot: () => PanelSnapshot | null } }).options.snapshot();
  const render = (tab: "Context" | "Plan") => {
    const text = stripTerminalSequences((tab === "Plan" ? panels.renderSidebar(110, 200) : activityLines(snapshot(), 0)).join("\n"));
    captured.push(tab, text);
    return text;
  };
  const child = async (id: string, code: string) => fx.turn([
    { toolCalls: [{ callId: id, name: "codemode", arguments: { code } }] }, { text: "Finished" },
  ]);
  try {
    fx.corral.rules({ progress: { receipt: { state: "committed", result: { progress: progress(1) } } } });
    await fx.turn([{ toolCalls: [{ callId: "direct-progress", name: "corral_progress", arguments: { update: update(1) } }] }, { text: "Finished" }]);
    expect(render("Plan")).toContain("revision 1");

    fx.corral.rules({ ask: [
      { match: { text: "replay" }, receipt: { state: "committed", replayed: true, result: { key: "T-683/R1/Q1" } } },
      { match: { text: "reject" }, reject: { code: "stale_revision", reason: "item changed" } },
      { match: { text: "uncertain" }, receipt: { state: "uncertain", result: { code: "outcome_unknown" } } },
    ], progress: { receipt: { state: "committed", result: { progress: progress(2) } } } });
    await child("nested-outcomes", `
      await tools.corral_progress({ update: ${JSON.stringify(update(2))} });
      await Promise.allSettled(["commit", "replay", "reject", "uncertain"].map(text => tools.corral_ask({kind: "clarification", text})));
    `);
    const context = render("Context");
    for (const outcome of ["committed", "replayed", "rejected", "uncertain"]) expect(context).toContain(outcome);
    expect(context).toContain("Code Mode child");
    expect(context).not.toContain("No tool receipt observed");
    const plan = render("Plan");
    expect(plan).toContain("Work revision 2");
    expect(plan).not.toContain("stale");
    expect(fx.corral.calls().find(call => call.op === "progress" && call.payload.expected_revision === 1)?.identity.parent?.call).toBeDefined();
    expect(fx.journal().some(entry => entry.tool === "corral_progress" && entry.phase === "settled" && entry.effect === "committed")).toBeTrue();

    // Todo uses the same canonical adapter as progress; a late replay cannot regress it.
    fx.corral.rules({ todo: { receipt: { state: "committed", replayed: true, result: { progress: progress(1) } } } });
    await child("late-todo", 'await tools.corral_todo({ op: "start", step: "s" });');
    expect(render("Plan")).toContain("revision 2");
    fx.corral.rules({ todo: { reject: { code: "stale_revision", reason: "changed" } } });
    await child("rejected-todo", 'await tools.corral_todo({ op: "done", step: "s" });');
    expect(render("Plan")).toContain("revision 2"); expect(render("Plan")).not.toContain("stale");
    fx.corral.rules({ todo: { receipt: { state: "uncertain", result: { code: "outcome_unknown" } } } });
    await child("uncertain-todo", 'await tools.corral_todo({ op: "done", step: "s" });');
    expect(render("Plan")).toContain("stale"); expect(render("Plan")).toContain("revision 2");

    fx.corral.snapshot({ runs: [{ id: "R1", progress: progress(3) }] });
    fx.corral.rules({ todo: { receipt: { state: "committed", result: {} } } });
    await child("reconcile-todo", 'await tools.corral_todo({ op: "done", step: "s" });');
    expect(render("Plan")).toContain("revision 3"); expect(render("Plan")).not.toContain("stale");
    render("Context");
    const evidence = resolve(import.meta.dirname, "../../../.evidence");
    mkdirSync(evidence, { recursive: true });
    writeFileSync(resolve(evidence, "T-689-nested-host-render.txt"), captured.join("\n"));
  } finally { await fx.close(); }
}, 120_000);
