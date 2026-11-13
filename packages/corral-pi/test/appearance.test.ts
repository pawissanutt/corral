import { expect, test } from "bun:test";
import { startPanelHost } from "./support/panel-host.ts";

// Production HostSession + InteractiveMode, offline provider/control-plane transport only.
test("action catalog retains role restrictions and packaged preferences change rendering, not binding or draft", async () => {
  const h = await startPanelHost();
  try {
    for (const char of "Keep my draft 界面") h.physical.send(char);
    await h.physical.until("Keep my draft");
    const bound = h.host.sessionId();
    const actions = h.panels.actions();
    const answer = actions.find(a => a.id === "inbox.answer-flow");
    expect(answer?.disabled()).toContain("Role worker cannot answer");
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
      expect(h.panels.actions().find(a => a.id === choice?.id)?.label).toContain("(current)");
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
