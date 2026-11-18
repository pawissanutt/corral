import { afterEach, expect, test } from "bun:test";
import { Theme } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { ActionPalette, type ActionGroup, type HostAction } from "./action-palette.ts";
import { PACKAGED } from "./appearance.ts";
import { panelTheme } from "./panels-theme.ts";
import { box, setPanelAppearance, styledPanelLine } from "./panels-view.ts";
import { PanelCanvas, setCanvasAppearance } from "./panel-canvas.ts";
import { resourceLoaderOptions, restrictedSettings } from "../host/restrict.ts";
import { NativePanels } from "./panels.ts";

const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
const action = (id: string, disabled?: string, group: ActionGroup = "inspect"): HostAction => ({ id, label: id, shortcut: "F6",
  scope: "worker · demo", group, disabled: () => disabled, run() {} });
const click = (y: number): TuiMouseEvent => ({ type: "click", button: "left", x: 4, y, screenX: 4, screenY: y,
  width: 78, height: 12, shift: false, alt: false, ctrl: false });
afterEach(() => { setPanelAppearance("dark"); setCanvasAppearance("dark"); });

test("search uses labels, shortcuts, scope and disabled reasons; cancel never dispatches", () => {
  let chosen: HostAction | undefined;
  const palette = new ActionPalette([action("inspect"), action("answer", "Role worker cannot answer")],
    a => { chosen = a; }, () => {});
  palette.focused = true;
  palette.handleInput("worker answer");
  expect(palette.matches.map(a => a.id)).toEqual(["answer"]);
  expect(stripTerminalSequences(palette.render(80).join("\n"))).toContain("Role worker cannot answer");
  palette.handleInput("\r"); expect(chosen).toBeUndefined();
  palette.handleInput("\x1b"); expect(chosen).toBeUndefined();
  // A group's title matches too, and stays above its rows.
  const grouped = new ActionPalette([action("chat", undefined, "views"), action("keys", undefined, "help")], () => {}, () => {});
  grouped.handleInput("help");
  expect(grouped.matches.map(a => a.id)).toEqual(["keys"]);
  expect(plain(grouped.render(78)).slice(3, 5).map(line => line.includes("Help"))).toEqual([true, false]);
});

test("keyboard selection returns only explicit handlers; dynamic disabled state is checked", () => {
  let blocked = false, chosen: HostAction | undefined;
  const selectable = { ...action("model"), disabled: () => blocked ? "Capability unavailable" : undefined };
  const palette = new ActionPalette([action("inspect"), selectable], a => { chosen = a; }, () => {});
  palette.handleInput("\x1b[B"); blocked = true;
  palette.handleInput("\r"); expect(chosen).toBeUndefined();
  blocked = false; palette.handleInput("\r"); expect(chosen).toBe(selectable);
});

test("palette clips wide glyphs and state labels at minimum widths and supports no-match search", () => {
  const palette = new ActionPalette([action("界面 👩‍💻 inspect"), action("cycle effort", "Effort unavailable")], () => {}, () => {});
  for (const width of [1, 20, 52, 80]) for (const line of palette.render(width)) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
  palette.handleInput("unknown action"); expect(palette.matches).toHaveLength(0);
  expect(plain(palette.render(52)).some(line => line.startsWith("│▌"))).toBeFalse();
});

test("rows sit under non-selectable group titles in group order; a setting in force is marked current", () => {
  const display = { ...action("motion.still", undefined, "display"), current: true };
  const palette = new ActionPalette([action("inspect"), display, action("chat", undefined, "views"), action("answer", "Role worker cannot answer")],
    () => {}, () => {});
  expect(palette.matches.map(a => a.id)).toEqual(["chat", "inspect", "answer", "motion.still"]);
  const text = plain(palette.render(78));
  expect(text[0]!.startsWith("╭")).toBeTrue();
  expect(text[1]).toMatch(/^│ › .*type to filter actions.* worker · demo │$/);
  expect(text[2]).toMatch(/^│─+│$/);
  // The title takes a row without the bar; the first action under it is selected.
  expect(text.slice(3, 5).map(line => line.startsWith("│▌"))).toEqual([false, true]);
  // The setting in force is marked on its row, not in its label.
  const marked = (current: boolean) => plain(new ActionPalette([{ ...display, current }], () => {}, () => {}).render(78))
    .find(line => line.includes("motion.still"))!;
  expect(marked(true)).not.toBe(marked(false));
  expect(palette.matches.find(a => a.current)?.label).toBe("motion.still");
  expect(text.at(-1)!.startsWith("╰")).toBeTrue();
  for (const line of palette.render(78)) expect(visibleWidth(line)).toBe(78);
});

test("titles count against the window; arrows, wheel and clicks reach the action drawn there, never a title", () => {
  let chosen: HostAction | undefined;
  const views = [action("a", undefined, "views"), action("b", undefined, "views")], inspect = [action("c"), action("d")];
  const palette = new ActionPalette([...views, ...inspect], a => { chosen = a; }, () => {}, 4);
  expect(plain(palette.render(78)).slice(3, -1)).toHaveLength(4);
  palette.handleMouse(click(3)); expect(chosen).toBeUndefined();
  palette.handleMouse(click(5)); expect(chosen).toBeUndefined();
  palette.handleMouse(click(5)); expect(chosen).toBe(views[1]);
  palette.handleMouse({ ...click(5), type: "wheel", button: "none", wheelDelta: 2 });
  const window = plain(palette.render(78)).slice(3, -1);
  const row = window.findIndex(line => /^│.d +F6 │$/.test(line));
  expect(row).toBeGreaterThan(0);
  palette.handleMouse(click(3 + row)); palette.handleMouse(click(3 + row)); expect(chosen).toBe(inspect[1]);
  // ↓ wraps to the first action and brings its title back into the window.
  palette.handleInput("\x1b[B");
  expect(plain(palette.render(78)).slice(3, 5).map(line => line.startsWith("│▌"))).toEqual([false, true]);
  palette.handleInput("\r"); expect(chosen).toBe(views[0]);
});

test("production catalog disables effort when capability is unavailable or unknown", () => {
  const panels = new NativePanels({ identity: { store: "st-0123456789abcdef", project: "demo", role: "worker" },
    cwd: process.cwd(), snapshot: () => null });
  try {
    const effort = panels.actions().find(a => a.id === "app.thinking.cycle");
    const reason = effort?.disabled();
    expect(reason).toBeDefined();
    const palette = new ActionPalette(panels.actions(), () => { throw new Error("disabled action dispatched"); }, () => {});
    palette.handleInput("app.thinking.cycle");
    expect(stripTerminalSequences(palette.render(100).join("\n"))).toContain(reason!);
    palette.handleInput("\r");
  } finally { panels.dispose(); }
});

test("presentation adds no ambient resources", () => {
  const resources = resourceLoaderOptions([]);
  expect(resources.noThemes).toBeTrue(); expect(resources.noExtensions).toBeTrue();
  expect(resources.noPromptTemplates).toBeTrue(); expect(resources.noSkills).toBeTrue();
  expect(resources.extensionFactories).toEqual([]);
  expect(restrictedSettings().extensions).toEqual([]);
});

test("appearance changes seed new canvas defaults before full-frame erases", () => {
  const canvas = new PanelCanvas();
  canvas.write("\x1b[?1049h\x1b[0mold appearance");
  setPanelAppearance("light"); setCanvasAppearance("light");
  const repaint = canvas.write("\x1b[?2026h\x1b[2J").data;
  expect(repaint.startsWith("\x1b[38;2;23;33;47m\x1b[48;2;255;255;255m")).toBeTrue();
  expect(repaint.indexOf("48;2;255;255;255")).toBeLessThanOrEqual(repaint.indexOf("[2J") - 1);
  expect(canvas.write("\x1b[?2026h\x1b[2J").data).toBe("\x1b[?2026h\x1b[2J");
});

function luminance(hex: string): number {
  const rgb = [1, 3, 5].map(i => Number.parseInt(hex.slice(i, i + 2), 16) / 255)
    .map(c => c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return rgb[0]! * 0.2126 + rgb[1]! * 0.7152 + rgb[2]! * 0.0722;
}
for (const appearance of ["dark", "light", "contrast"] as const) {
  test(`${appearance}: text/state contrast, semantic labels, bounded panels and canvas reset colors`, () => {
    const p = PACKAGED[appearance];
    for (const bg of [p.background, p.selected]) for (const fg of [p.text, p.muted, p.accent, p.success, p.warning, p.error]) {
      const a = luminance(bg), b = luminance(fg);
      expect((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)).toBeGreaterThanOrEqual(4.5);
    }
    setPanelAppearance(appearance); setCanvasAppearance(appearance);
    const sdkTheme = panelTheme(Theme, "truecolor", appearance);
    expect(sdkTheme.appearance).toBe(appearance === "light" ? "light" : "dark");
    const rows = [styledPanelLine({ text: "Unbound · unavailable", tone: "warning" }),
      styledPanelLine({ text: "Bound · confirmed", tone: "success" })];
    expect(stripTerminalSequences(rows.join("\n"))).toContain("Unbound · unavailable");
    for (const width of [20, 52, 80]) for (const row of box("Selected · Files", rows, width, 5, true)) expect(visibleWidth(row)).toBe(width);
    const canvas = new PanelCanvas();
    expect(canvas.write("\x1b[?1049h\x1b[0m").data).toContain(p.background.slice(1, 3) === "ff" ? "48;2;255;255;255" : "48;2;");
  });
}
