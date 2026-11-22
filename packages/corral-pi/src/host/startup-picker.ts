import { Input, Key, matchesKey, ProcessTerminal, SelectList, truncateToWidth, TuiMainScreen,
  type Component, type Focusable, type SelectItem } from "@earendil-works/pi-tui";
import type { StartupModel } from "./startup-model.ts";

interface StartupSelection {
  readonly model: StartupModel;
  readonly save: boolean;
}

const plain = (text: string) => text;
const theme = {
  selectedPrefix: plain, selectedText: plain, description: plain, scrollInfo: plain,
  noMatch: () => "No matching models",
};

/** Own the terminal only until a choice is confirmed; the session TUI has not been started. */
export async function pickStartupModel(models: readonly StartupModel[], title: string,
  saveIn?: "workspace" | "personal"): Promise<StartupSelection | undefined> {
  const terminal = new ProcessTerminal();
  const tui = new TuiMainScreen(terminal);
  const search = new Input();
  let model: StartupModel | undefined;
  let list: SelectList;
  const chose = Promise.withResolvers<StartupSelection | undefined>();
  function show(items: SelectItem[], onSelect: (item: SelectItem) => void): SelectList {
    const visible = Math.max(1, Math.min(8, terminal.rows - 7));
    const next = new SelectList(items, visible, theme);
    next.onSelect = onSelect;
    return next;
  }
  function showModels() {
    const terms = search.getValue().toLowerCase().trim().split(/\s+/);
    const filtered = models.filter(item => terms.every(term => `${item.selector} ${item.label}`.toLowerCase().includes(term)));
    list = show(filtered.map(item => ({ value: item.selector, label: item.selector })), item => {
      model = filtered.find(candidate => candidate.selector === item.value)!;
      list = show([
        { value: "once", label: "Use once" },
        ...(saveIn && model.savable ? [{ value: "save", label: `Use and save as ${saveIn} default` }] : []),
        { value: "cancel", label: "Cancel" },
      ], action => chose.resolve(action.value === "cancel" ? undefined : { model: model!, save: action.value === "save" }));
    });
  }
  showModels();
  const picker: Component & Focusable = {
    get focused() { return search.focused; },
    set focused(value: boolean) { search.focused = value; },
    render(width) {
      if (width < 5) return [];
      const lines = model ? [`Use ${model.selector}`, model.label, ...list.render(width), "↑↓ move · Enter confirms · Esc cancels"]
        : [title, "Search models", ...search.render(width), ...list.render(width),
          models.find(candidate => candidate.selector === list.getSelectedItem()?.value)?.label ?? "",
          "↑↓ move · Enter selects · Esc cancels"];
      return lines.map(line => truncateToWidth(line, width));
    },
    handleInput(data) {
      if (matchesKey(data, Key.escape) || matchesKey(data, Key.ctrl("c"))) {
        chose.resolve(undefined);
      } else if (model || ([Key.up, Key.down, Key.pageUp, Key.pageDown, Key.enter] as const).some(key => matchesKey(data, key))) {
        list.handleInput(data);
      } else {
        const before = search.getValue();
        search.handleInput(data);
        if (search.getValue() !== before) showModels();
      }
      tui.requestRender();
    },
    invalidate() { search.invalidate(); list.invalidate(); },
  };
  tui.addChild(picker);
  tui.setFocus(picker);
  try {
    tui.start();
    return await chose.promise;
  } finally { tui.stop(); }
}
