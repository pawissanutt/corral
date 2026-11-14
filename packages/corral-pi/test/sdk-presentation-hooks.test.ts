// The patched SDK's presentation hooks draw Pi's surfaces, but Pi keeps their outcome. Dialogs: the promise, abort,
// timeout, settle-once and the `Yes` comparison of confirm, run through Pi's own dialog methods on a host whose dialog
// stack is recorded. Nested settings lists: Pi's own items, change dispatch, preview rollback and fallback. Every case
// is deterministic and needs no terminal.
import { beforeAll, expect, jest, test } from "bun:test";
import {
  ExtensionSelectorComponent, initTheme, InteractiveMode, type DialogDone, type DialogPresentation, type InputDialogRequest,
  type OptionSelectorRequest, type PresentedDialog, type SelectDialogRequest, type SelectorComponent, type SettingsCallbacks,
  type SettingsConfig, type SettingsListPresentation, type SettingsListRequest,
} from "@earendil-works/pi-coding-agent";
import { SettingsList, type SettingItem } from "@earendil-works/pi-tui";
import { createSettingsModel } from "../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/components/settings-selector.js";

beforeAll(() => initTheme("dark", false));

class Dialog implements PresentedDialog {
  readonly countdowns: number[] = [];
  disposed = 0;
  render(): string[] { return []; }
  invalidate(): void {}
  handleInput(): void {}
  setCountdown(seconds: number): void { this.countdowns.push(seconds); }
  dispose(): void { this.disposed++; }
}

interface Opened<R> { readonly request: R; readonly done: DialogDone; readonly dialog: Dialog }

/** An InteractiveMode whose editor slot only records what Pi pushes and removes. */
function dialogHost(dialogPresentation: DialogPresentation) {
  const pushed: object[] = [], removed: object[] = [], blocked: unknown[] = [];
  const host: object = Object.create(InteractiveMode.prototype);
  Object.assign(host, {
    options: { dialogPresentation },
    ui: { requestRender() {} },
    toggleToolOutputExpansion() {},
    pushExtensionDialog(component: object, status: unknown) { pushed.push(component); blocked.push(status); },
    removeExtensionDialog(component: object) { removed.push(component); },
  });
  const call = (name: "showExtensionSelector" | "showExtensionConfirm" | "showExtensionInput", ...args: unknown[]): Promise<unknown> => {
    const method: unknown = Reflect.get(InteractiveMode.prototype, name);
    if (typeof method !== "function") throw new Error(`InteractiveMode.${name} is missing`);
    return Promise.resolve(Reflect.apply(method, host, args));
  };
  return { call, pushed, removed, blocked };
}

function selectHost() {
  const opened: Opened<SelectDialogRequest>[] = [];
  const host = dialogHost({
    select(request, done) {
      const dialog = new Dialog();
      opened.push({ request, done, dialog });
      return dialog;
    },
  });
  return { ...host, opened };
}

test("a pre-aborted dialog resolves undefined without asking the host", async () => {
  const host = selectHost(), controller = new AbortController();
  controller.abort();
  expect(await host.call("showExtensionSelector", "Pick", ["a", "b"], { signal: controller.signal })).toBeUndefined();
  expect(host.opened).toHaveLength(0);
  expect(host.pushed).toHaveLength(0);
});

test("an abort while the host's dialog is open resolves undefined and closes it once", async () => {
  const host = selectHost(), controller = new AbortController();
  const pending = host.call("showExtensionSelector", "Pick", ["a", "b"], { signal: controller.signal });
  const [open] = host.opened;
  expect(host.pushed).toEqual([open?.dialog]);
  controller.abort();
  open?.done("a");
  expect(await pending).toBeUndefined();
  expect(open?.dialog.disposed).toBe(1);
  expect(host.removed).toEqual([open?.dialog]);
});

test("Pi's timeout drives the host's countdown and resolves undefined", async () => {
  const host = selectHost();
  jest.useFakeTimers();
  try {
    const pending = host.call("showExtensionSelector", "Pick", ["a", "b"], { timeout: 2_000 });
    const [open] = host.opened;
    expect(open?.request.timeoutMs).toBe(2_000);
    expect(open?.dialog.countdowns).toEqual([2]);
    jest.advanceTimersByTime(2_000);
    expect(await pending).toBeUndefined();
    expect(open?.dialog.countdowns).toEqual([2, 1, 0]);
    expect(open?.dialog.disposed).toBe(1);
  } finally {
    jest.useRealTimers();
  }
});

test("an option that was not offered resolves undefined, and only the first answer counts", async () => {
  const host = selectHost();
  const invented = host.call("showExtensionSelector", "Pick", ["a", "b"]);
  host.opened[0]?.done("c");
  host.opened[0]?.done("a");
  expect(await invented).toBeUndefined();

  const chosen = host.call("showExtensionSelector", "Pick", ["a", "b"]);
  host.opened[1]?.done("b");
  host.opened[1]?.done("a");
  expect(await chosen).toBe("b");
  expect(host.removed).toEqual([host.opened[0]?.dialog, host.opened[1]?.dialog]);
});

test("confirm reaches the host as a confirm and only Yes confirms", async () => {
  const host = selectHost();
  const declined = host.call("showExtensionConfirm", "Delete the branch?", "It has 3 unmerged commits.");
  const request = host.opened[0]?.request;
  expect(request?.kind).toBe("confirm");
  expect(request?.title).toBe("Delete the branch?");
  expect(request?.message).toBe("It has 3 unmerged commits.");
  expect(request?.options).toEqual(["Yes", "No"]);
  expect(host.blocked).toEqual([{ kind: "permission", message: "Delete the branch?" }]);
  host.opened[0]?.done(undefined);
  expect(await declined).toBe(false);

  const accepted = host.call("showExtensionConfirm", "Delete the branch?", "It has 3 unmerged commits.");
  host.opened[1]?.done("Yes");
  expect(await accepted).toBe(true);
});

test("a hook that throws leaves Pi's own dialog, and the hook's done no longer answers", async () => {
  const escaped: { done?: DialogDone } = {};
  const host = dialogHost({ select(_request, done) { escaped.done = done; throw new Error("presenter failed"); } });
  const controller = new AbortController();
  const pending = host.call("showExtensionSelector", "Pick", ["a", "b"], { signal: controller.signal });
  expect(host.pushed[0] instanceof ExtensionSelectorComponent).toBe(true);
  escaped.done?.("a");
  controller.abort();
  expect(await pending).toBeUndefined();
});

test("input resolves the host's text once; no timeout means no countdown", async () => {
  const opened: Opened<InputDialogRequest>[] = [];
  const host = dialogHost({
    input(request, done) {
      const dialog = new Dialog();
      opened.push({ request, done, dialog });
      return dialog;
    },
  });
  const pending = host.call("showExtensionInput", "Name", "branch name");
  expect(opened[0]?.request.placeholder).toBe("branch name");
  expect(opened[0]?.request.timeoutMs).toBeUndefined();
  opened[0]?.done("");
  opened[0]?.done("feature");
  expect(await pending).toBe("");
  expect(opened[0]?.dialog.countdowns).toEqual([]);
  expect(host.removed).toEqual([opened[0]?.dialog]);
});

type WarningSettings = Parameters<SettingsCallbacks["onWarningsChange"]>[0];

function settingsConfig(currentTheme: string): SettingsConfig {
  return {
    autoCompact: true, defaultModel: "not set", availableDefaultModels: [], showImages: false, imageWidthCells: 80,
    autoResizeImages: true, blockImages: false, enableSkillCommands: true, steeringMode: "all", followUpMode: "all",
    transport: "sse", sessionShareDestination: "auto", httpIdleTimeoutMs: 300_000, cacheWarmingMode: "off",
    thinkingLevel: "off", availableThinkingLevels: ["off"], modelThinkingLevels: {}, currentTheme, terminalTheme: "dark",
    availableThemes: ["dark", "light"], hideThinkingBlock: false, mermaidRenderingMode: "off", showCacheMissNotices: false,
    collapseChangelog: false, enableInstallTelemetry: false, doubleEscapeAction: "tree", treeFilterMode: "default",
    showHardwareCursor: false, editorPaddingX: 0, outputPad: 1, autocompleteMaxVisible: 5, quietStartup: false,
    defaultProjectTrust: "ask", clearOnShrink: false, showTerminalProgress: false, tuiMode: "regular",
    fullscreenExitOutput: "transcript", fullscreenScrollbar: "auto", fullscreenCopyOnSelect: false,
    fullscreenWheelScrollLines: "auto", warnings: { anthropicExtraUsage: true },
  };
}

function settingsCallbacks(overrides: Partial<SettingsCallbacks>): SettingsCallbacks {
  const ignore = () => {};
  return {
    onAutoCompactChange: ignore, onShowImagesChange: ignore, onImageWidthCellsChange: ignore, onAutoResizeImagesChange: ignore,
    onBlockImagesChange: ignore, onEnableSkillCommandsChange: ignore, onSteeringModeChange: ignore, onFollowUpModeChange: ignore,
    onTransportChange: ignore, onSessionShareDestinationChange: ignore, onHttpIdleTimeoutMsChange: ignore,
    onCacheWarmingModeChange: ignore, onModelThinkingLevelChange: ignore, onModelThinkingLevelRemove: ignore,
    onThemeChange: ignore, onHideThinkingBlockChange: ignore, onMermaidRenderingModeChange: ignore,
    onShowCacheMissNoticesChange: ignore, onCollapseChangelogChange: ignore, onEnableInstallTelemetryChange: ignore,
    onDoubleEscapeActionChange: ignore, onTreeFilterModeChange: ignore, onShowHardwareCursorChange: ignore,
    onEditorPaddingXChange: ignore, onOutputPadChange: ignore, onAutocompleteMaxVisibleChange: ignore,
    onQuietStartupChange: ignore, onDefaultProjectTrustChange: ignore, onClearOnShrinkChange: ignore,
    onShowTerminalProgressChange: ignore, onTuiModeChange: ignore, onFullscreenExitOutputChange: ignore,
    onFullscreenScrollbarChange: ignore, onFullscreenCopyOnSelectChange: ignore, onFullscreenWheelScrollLinesChange: ignore,
    onWarningsChange: ignore, onCancel: ignore,
    ...overrides,
  };
}

class NestedList implements SelectorComponent {
  focused = false;
  disposed = 0;
  render(): string[] { return []; }
  invalidate(): void {}
  handleInput(): void {}
  dispose(): void { this.disposed++; }
}

function nestedHost() {
  const requests: SettingsListRequest[] = [], lists: NestedList[] = [];
  const present: SettingsListPresentation = request => {
    const list = new NestedList();
    requests.push(request);
    lists.push(list);
    return list;
  };
  return { present, requests, lists };
}

/** Opens the top-level item `id`'s Pi submenu, recording what its `done` returns. */
function openSubmenu(items: readonly SettingItem[], id: string) {
  const closed: (string | undefined)[] = [];
  const item = items.find(candidate => candidate.id === id);
  const submenu = item?.submenu?.(item.currentValue, selected => { closed.push(selected); });
  if (!submenu) throw new Error(`setting ${id} has no submenu`);
  return { submenu, closed };
}

test("the host's Warnings list changes Pi's warning state, which the next opening keeps; focus and cleanup reach it", () => {
  const host = nestedHost(), changes: WarningSettings[] = [];
  const model = createSettingsModel(settingsConfig("dark"), settingsCallbacks({ onWarningsChange: warnings => { changes.push(warnings); } }),
    undefined, host.present);
  const first = openSubmenu(model.items, "warnings");
  const request = host.requests[0];
  expect(request?.title).toBe("Warnings");
  expect(request?.items.map(item => [item.id, item.currentValue])).toEqual([["anthropic-extra-usage", "true"]]);
  request?.change("anthropic-extra-usage", "false");
  expect(changes).toEqual([{ anthropicExtraUsage: false }]);
  expect(request?.items[0]?.currentValue).toBe("false");
  Reflect.set(first.submenu, "focused", true);
  expect(host.lists[0]?.focused).toBe(true);
  request?.cancel();
  expect(host.lists[0]?.disposed).toBe(1);
  expect(first.closed).toEqual([undefined]);

  openSubmenu(model.items, "warnings");
  expect(host.requests[1]?.items[0]?.currentValue).toBe("false");
});

test("a nested hook that throws leaves Pi's own Warnings list", () => {
  const model = createSettingsModel(settingsConfig("dark"), settingsCallbacks({}), undefined, () => { throw new Error("presenter failed"); });
  const { submenu } = openSubmenu(model.items, "warnings");
  const children: unknown = Reflect.get(submenu, "children");
  expect(Array.isArray(children) && children.length === 1 && children[0] instanceof SettingsList).toBe(true);
});

test("the host's Automatic Theme list applies, switches mode and cancels through Pi's own theme callbacks", () => {
  const previews: string[] = [];
  const callbacks = settingsCallbacks({ onThemePreview: setting => { previews.push(setting); } });

  const applying = nestedHost();
  const applied = openSubmenu(createSettingsModel(settingsConfig("light/dark"), callbacks, undefined, applying.present).items, "theme");
  expect(applying.requests[0]?.title).toBe("Automatic Theme");
  expect(applying.requests[0]?.items.map(item => item.id)).toEqual(["light-theme", "dark-theme", "apply", "single-mode"]);
  applying.requests[0]?.change("apply", "save and go back");
  expect(applied.closed).toEqual(["light/dark"]);
  expect(applying.lists[0]?.disposed).toBe(1);

  const switching = nestedHost();
  const switched = openSubmenu(createSettingsModel(settingsConfig("light/dark"), callbacks, undefined, switching.present).items, "theme");
  switching.requests[0]?.change("single-mode", "switch to single theme");
  expect(switching.lists[0]?.disposed).toBe(1);
  expect(previews.at(-1)).toBe("dark");
  expect(switched.closed).toEqual([]);

  const cancelling = nestedHost();
  const cancelled = openSubmenu(createSettingsModel(settingsConfig("light/dark"), callbacks, undefined, cancelling.present).items, "theme");
  cancelling.requests[0]?.cancel();
  expect(previews.at(-1)).toBe("light/dark");
  expect(cancelled.closed).toEqual([undefined]);
  expect(cancelling.lists[0]?.disposed).toBe(1);
});

test("per-model thinking steps hand focus to the host's option list and dispose each step they leave once", () => {
  const requests: OptionSelectorRequest[] = [], lists: NestedList[] = [];
  const presentOptions = (request: OptionSelectorRequest) => {
    const list = new NestedList();
    requests.push(request);
    lists.push(list);
    return list;
  };
  const model = createSettingsModel(settingsConfig("dark"), settingsCallbacks({}), presentOptions);
  const { submenu, closed } = openSubmenu(model.items, "model-thinking");
  Reflect.set(submenu, "focused", true);
  expect(lists[0]?.focused).toBe(true);

  expect(requests[0]?.items.map(item => item.value)).toEqual(["__none__"]);
  requests[0]?.select("__none__");
  expect(lists[0]?.disposed).toBe(1);
  expect(lists[1]?.focused).toBe(true);

  requests[1]?.cancel();
  expect(lists[1]?.disposed).toBe(1);
  expect(lists[2]?.focused).toBe(true);

  requests[2]?.cancel();
  expect(lists[2]?.disposed).toBe(1);
  expect(closed).toEqual(["none"]);
  const dispose: unknown = Reflect.get(submenu, "dispose");
  if (typeof dispose === "function") Reflect.apply(dispose, submenu, []);
  expect(lists.map(list => list.disposed)).toEqual([1, 1, 1]);
});
