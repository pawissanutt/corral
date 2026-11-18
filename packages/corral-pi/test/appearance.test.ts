import { expect, test } from "bun:test";
import { NativePanels } from "../src/ui/panels.ts";
import { startPanelHost } from "./support/panel-host.ts";

// Production HostSession + InteractiveMode, offline provider/control-plane transport only.
test("action catalog retains role restrictions and packaged preferences change rendering, not binding or draft", async () => {
  const h = await startPanelHost();
  try {
    for (const char of "Keep my draft 界面") h.physical.send(char);
    await h.physical.until("Keep my draft");
    const bound = h.host.sessionId();
    const actions = h.panels.actions();
    // Rewind is offered to the worker role only: available here, absent for a reviewer even when the host supplies it.
    const rewind = actions.find(a => a.id === "rewind");
    expect(rewind).toBeDefined();
    expect(rewind?.disabled()).toBeUndefined();
    const reviewer = new NativePanels({ identity: { store: "st-0123456789abcdef", project: "demo", role: "reviewer", task: "T-001", run: "R2" },
      cwd: h.work, snapshot: () => null, rewind: async () => ({}) });
    try { expect(reviewer.actions().some(a => a.id === "rewind")).toBeFalse(); } finally { reviewer.dispose(); }
    expect(actions.every(a => a.scope === "worker · demo")).toBeTrue();
    expect(actions.find(a => a.id === "app.model.select")?.shortcut.toLowerCase()).toContain("ctrl+l");
    expect(actions.some(a => /shell|exec|permission|role\.switch/.test(a.id))).toBeFalse();
    const draftColumn = () => h.screen().find("Keep my draft")?.col ?? -1;
    const comfortable = draftColumn();
    await actions.find(a => a.id === "density.compact")?.run();
    expect(h.panels.density).toBe("compact");
    // Compact density drops the composer's inner padding; the draft stays where it was typed.
    await h.physical.whenWritten(() => draftColumn() >= 0 && draftColumn() < comfortable, "the compact composer");
    for (const appearance of ["light", "contrast", "dark"]) {
      const choice = h.panels.actions().find(a => a.id === `appearance.${appearance}`);
      expect(choice?.disabled()).toBeUndefined();
      await choice?.run();
      expect(h.panels.appearance).toBe(appearance);
      const current = h.panels.actions().filter(a => a.id.startsWith("appearance.") && a.current).map(a => a.id);
      expect(current).toEqual([`appearance.${appearance}`]);
    }
    // Main's Usage command must survive extraction into the shared palette/command handler.
    const usage = h.panels.actions().find(a => a.id === "/panels usage");
    expect(usage?.disabled()).toBeUndefined();
    await usage?.run();
    expect(h.panels.workbench.view).toBe("usage");
    expect(h.host.sessionId()).toBe(bound);
    expect(h.screen().text()).toContain("Keep my draft");
  } finally { await h.stop(); }
});
