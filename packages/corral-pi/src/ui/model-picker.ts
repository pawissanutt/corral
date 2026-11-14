// Pi's model, effort and option pickers in the owner card's list form, in the composer's place. Pi keeps what a choice
// does: every commit is the request's own `select` with Pi's persist flag, every close its `cancel`, and Pi's status or
// error for it (a refused switch leaves the model as it was) reaches the owner through Pi's notices. A picker commits
// or cancels at most once; afterwards keys and clicks are ignored until Pi closes it.
import type { Api, Model } from "@earendil-works/pi-ai";
import type { ModelSelectorRequest, OptionSelectorRequest, SelectorComponent, ThinkingSelectorRequest } from "@earendil-works/pi-coding-agent";
import { visibleWidth } from "@earendil-works/pi-tui";
import type { TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";
import { keyData, keyLabel, piKey, piPressed } from "./keymap.ts";
import { ownerListCard, type OwnerDialogHost, type OwnerListItem, type OwnerListOptions } from "./owner-dialog.ts";
import type { DialogHint } from "./owner-dialog-parts.ts";
import { PANEL_BLUE, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, plainLabel } from "./panels-view.ts";
import { cell, detailRows, GAP, rowText } from "./picker-rows.ts";

/** Pi's own selector gives up refreshing the catalogs after this long and shows the cached models. */
const REFRESH_TIMEOUT_MS = 15_000;
/** Column caps, so one long id or provider does not push every row's marks off the card. */
const ID_MAX = 40, PROVIDER_MAX = 18, LABEL_MAX = 32;
/** Narrowest id or note column worth drawing; below it the column gives way and the selected item's rows say it. */
const ID_MIN = 12, NOTE_MIN = 8;
/** Pi's own note when the session has no model scope. */
const PROVIDERS_NOTE = "Only showing models from configured providers. Use /login to add providers.";

/** The catalog refresh and what it found, one body row of the model card. */
interface RefreshStatus { readonly text: string; readonly tone: "muted" | "success" | "warning" }

interface PickerSpec<T> {
  readonly list: Omit<OwnerListOptions<T>, "key" | "done">;
  /** Pi's commit for Enter or a click on the selected item. */
  choose(value: T): void;
  cancel(): void;
  /** Keys before the card's: a commit to run once (save as default), or true when handled without one. */
  key?(data: string, selected: T | undefined): (() => void) | boolean;
  /** Runs once, when the picker commits, cancels or Pi closes it. */
  stop?(): void;
}

/** A list card that commits or cancels once; afterwards keys and clicks are ignored until Pi closes it. */
class Picker<T> implements SelectorComponent {
  private readonly card: SelectorComponent;
  private live = true;

  constructor(private readonly spec: PickerSpec<T>, host: OwnerDialogHost) {
    this.card = ownerListCard<T>({
      ...spec.list,
      key: (data, selected) => {
        const handled = spec.key?.(data, selected) ?? false;
        if (typeof handled !== "function") return handled;
        this.settle(handled);
        return true;
      },
      done: value => this.settle(value === undefined ? () => spec.cancel() : () => spec.choose(value)),
    }, host);
  }

  get focused(): boolean { return this.card.focused ?? false; }
  set focused(value: boolean) { this.card.focused = value; }
  get capturesViewportInput(): boolean { return this.card.capturesViewportInput ?? false; }

  invalidate(): void { this.card.invalidate(); }

  handleInput(data: string): void {
    if (this.live) this.card.handleInput(data);
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    return this.live ? this.card.handleMouse?.(event) : { handled: true };
  }

  render(width: number): string[] { return this.card.render(width); }

  dispose(): void {
    this.close();
    this.card.dispose?.();
  }

  private close(): boolean {
    if (!this.live) return false;
    this.live = false;
    this.spec.stop?.();
    return true;
  }

  private settle(commit: () => void): void {
    if (this.close()) commit();
  }
}

/** The hint for a Pi binding, when the owner has it bound to a key a click can replay. */
function bindingHint(id: string, label: string): DialogHint[] {
  const key = piKey(id), data = key ? keyData(key) : undefined;
  return key && data ? [{ key: keyLabel(key), label, data }] : [];
}

/** A row's marks, styled, and the cells their plain text takes. */
interface Marks { readonly text: string; readonly width: number }

/** `current` in accent and `default` muted, as they apply. */
function marks(current: boolean, isDefault: boolean): Marks {
  const words = [...(current ? ["current"] : []), ...(isDefault ? ["default"] : [])];
  const text = (current ? PANEL_BLUE + "current" + PANEL_TEXT : "") + (current && isDefault ? PANEL_MUTED + " · " : "")
    + (isDefault ? PANEL_MUTED + "default" + PANEL_TEXT : "");
  return { text, width: visibleWidth(words.join(" · ")) };
}

/**
 * A label column, a muted note after it, marks at the end. A note too long for its room is clipped; on the selected
 * row it moves whole under the label instead, as does a clipped label.
 */
function labelledLines(label: string, column: number, note: string, mark: Marks, width: number, selected: boolean): string[] {
  const tail = mark.width ? mark.width + GAP.length : 0;
  const labelWidth = Math.max(1, Math.min(column, width - tail));
  const room = width - labelWidth - GAP.length - tail;
  const whole = !note || visibleWidth(note) <= room;
  const middle = note && room >= NOTE_MIN && (whole || !selected) ? PANEL_MUTED + cell(note, room) + PANEL_TEXT : " ".repeat(Math.max(0, room));
  const row = cell(label, labelWidth, selected) + GAP + middle + (mark.width ? GAP + mark.text : "");
  if (!selected) return [row];
  return [
    row,
    ...(visibleWidth(label) > labelWidth ? detailRows(label, width, 2, PANEL_TEXT) : []),
    ...(whole && room >= NOTE_MIN ? [] : detailRows(note, width)),
  ];
}

const modelKey = (model: { readonly provider: string; readonly id: string }) => `${model.provider}\0${model.id}`;

/**
 * Pi's model selector: the current model first, then the default, then by provider (Pi's order); Tab switches between
 * all models and the session's scoped models when it has some; Enter uses the selected model for this session and
 * Pi's save key also makes it the default. Pi's catalog refresh runs while the card is open, gives up after 15 s, and
 * is aborted when the picker commits, cancels or closes; the selection stays on its model when the rows change.
 */
export function modelPicker(request: ModelSelectorRequest, host: OwnerDialogHost): SelectorComponent {
  const current = request.current ? modelKey(request.current) : undefined;
  const defaultKey = request.defaultModel ? modelKey(request.defaultModel) : undefined;
  const rank = (model: Model<Api>) => modelKey(model) === current ? 0 : modelKey(model) === defaultKey ? 1 : 2;
  const sorted = (models: readonly Model<Api>[]) => [...models].sort((a, b) => rank(a) - rank(b) || a.provider.localeCompare(b.provider));

  let all = sorted(request.models());
  let scoped = request.scoped.map(entry => entry.model);
  let scope: "all" | "scoped" = scoped.length ? "scoped" : "all";
  let status: RefreshStatus = { text: "Refreshing model catalogs…", tone: "muted" };
  let stopped = false, timedOut = false;
  const controller = new AbortController();
  const timer = setTimeout(() => { timedOut = true; controller.abort(); }, REFRESH_TIMEOUT_MS);
  const cached = (why: string): RefreshStatus => ({ text: `${why}; showing cached models.`, tone: "warning" });
  void request.refresh(controller.signal).then(result => {
    if (stopped) return;
    const failed = [...result.errors.keys()];
    status = result.aborted && timedOut ? cached("Model refresh timed out")
      : failed.length === 1 ? cached(`Could not refresh ${failed[0]}`)
      : failed.length > 1 ? cached(`Could not refresh ${failed.length} model catalogs (${failed.join(", ")})`)
      : { text: "Model catalogs refreshed.", tone: "success" };
    all = sorted(request.models());
    scoped = request.scoped.map(entry => all.find(model => modelKey(model) === modelKey(entry.model)) ?? entry.model);
    host.changed();
  }, (error: unknown) => {
    if (stopped) return;
    status = timedOut ? cached("Model refresh timed out")
      : { text: `Could not refresh model catalogs: ${error instanceof Error ? error.message : String(error)}`, tone: "warning" };
    host.changed();
  }).finally(() => clearTimeout(timer));

  const shown = () => scope === "scoped" ? scoped : all;
  const find = (key: string | undefined) => key === undefined ? undefined : shown().find(model => modelKey(model) === key);
  const items = (): OwnerListItem<string>[] => {
    const models = shown();
    const idColumn = Math.min(ID_MAX, Math.max(1, ...models.map(model => visibleWidth(plainLabel(model.id)))));
    const providerColumn = Math.min(PROVIDER_MAX, Math.max(1, ...models.map(model => visibleWidth(plainLabel(model.provider)))));
    const markColumn = Math.max(0, ...models.map(model => marks(modelKey(model) === current, modelKey(model) === defaultKey).width));
    return models.map(model => {
      const key = modelKey(model), mark = marks(key === current, key === defaultKey);
      const id = plainLabel(model.id), provider = plainLabel(model.provider), name = plainLabel(model.name);
      return {
        value: key,
        match: `${provider} ${provider}/${id} ${id} ${name}${key === defaultKey ? " default" : ""}${key === current ? " current" : ""}`,
        lines: (width, selected) => {
          const tail = markColumn ? markColumn + GAP.length : 0;
          // The provider column goes before the id is cut below ID_MIN; the selected row's detail names it.
          const withProvider = width - tail - providerColumn - GAP.length >= Math.min(ID_MIN, idColumn);
          const idWidth = Math.max(1, Math.min(idColumn, width - tail - (withProvider ? providerColumn + GAP.length : 0)));
          const row = cell(id, idWidth, selected)
            + (withProvider ? GAP + PANEL_MUTED + cell(provider, providerColumn) + PANEL_TEXT : "")
            + (mark.width ? GAP + mark.text : "");
          if (!selected) return [row];
          return [
            row,
            ...detailRows(name && name !== id ? `${name} · ${provider}/${id}` : `${provider}/${id}`, width),
            ...(scoped.length ? [] : detailRows(PROVIDERS_NOTE, width, 2, PANEL_WARNING)),
          ];
        },
      };
    });
  };

  return new Picker<string>({
    list: {
      title: "Model",
      placeholder: "search models",
      ...(request.initialSearch ? { filter: request.initialSearch } : {}),
      ...(scoped.length ? { right: () => scope === "scoped" ? "scoped models" : "all models" } : {}),
      // The refresh status is the card's one body row; Pi's providers note sits in the selected model's detail, and
      // in the body only while there is no model to select.
      body: () => [
        (status.tone === "success" ? PANEL_SUCCESS : status.tone === "warning" ? PANEL_WARNING : PANEL_MUTED) + plainLabel(status.text) + PANEL_TEXT,
        ...(scoped.length || shown().length ? [] : [PANEL_WARNING + PROVIDERS_NOTE + PANEL_TEXT]),
      ],
      items,
      // Pi selects the current model, or the best match of its initial search.
      initial: rows => request.initialSearch ? 0 : Math.max(0, rows.findIndex(row => row.value === current)),
      hints: selected => [
        ...(find(selected) ? bindingHint("app.models.save", "make default") : []),
        ...(scoped.length ? bindingHint("tui.input.tab", scope === "all" ? "scoped models" : "all models") : []),
      ],
    },
    key: (data, selected) => {
      if (piPressed(data, "tui.input.tab")) {
        if (scoped.length) scope = scope === "all" ? "scoped" : "all";
        return true;
      }
      if (!piPressed(data, "app.models.save")) return false;
      const model = find(selected);
      return model ? () => request.select(model, true) : true;
    },
    choose: key => {
      const model = find(key);
      if (model) request.select(model, false); else request.cancel();
    },
    cancel: () => request.cancel(),
    stop: () => { stopped = true; clearTimeout(timer); controller.abort(); },
  }, host);
}

/** Pi's effort (thinking level) selector: its levels in Pi's order with Pi's descriptions; Pi's save key makes the default. */
export function effortPicker(request: ThinkingSelectorRequest, host: OwnerDialogHost): SelectorComponent {
  const cycle = piKey("app.thinking.cycle");
  const column = Math.max(1, ...request.levels.map(level => visibleWidth(level)));
  const items = request.levels.map((level): OwnerListItem<string> => {
    const mark = marks(level === request.current, level === request.defaultLevel), note = plainLabel(request.describe(level) ?? "");
    return {
      value: level,
      match: `${level} ${note}${level === request.defaultLevel ? " default" : ""}${level === request.current ? " current" : ""}`,
      lines: (width, selected) => labelledLines(level, column, note, mark, width, selected),
    };
  });
  const level = (value: string | undefined) => request.levels.find(item => item === value);
  return new Picker<string>({
    list: {
      title: "Effort", placeholder: "search levels",
      ...(cycle ? { right: () => `${keyLabel(cycle)} cycles in the composer` } : {}),
      items: () => items,
      initial: rows => rows.findIndex(row => row.value === request.current),
      hints: selected => level(selected) ? bindingHint("app.thinking.save", "make default") : [],
    },
    key: (data, selected) => {
      if (!piPressed(data, "app.thinking.save")) return false;
      const chosen = level(selected);
      return chosen ? () => request.select(chosen, true) : true;
    },
    choose: value => {
      const chosen = level(value);
      if (chosen) request.select(chosen, false); else request.cancel();
    },
    cancel: () => request.cancel(),
  }, host);
}

/**
 * One of the option lists Pi's settings open (a theme, a per-model effort): Pi's items, labels with Pi's own marks,
 * descriptions, and Pi's current value selected. Pi's preview follows the selection as the owner moves it; Esc is Pi's
 * back, which restores what it previewed.
 */
export function optionPicker(request: OptionSelectorRequest, host: OwnerDialogHost): SelectorComponent {
  const labels = request.items.map(item => rowText(item.label));
  const column = Math.min(LABEL_MAX, Math.max(1, ...labels.map(label => visibleWidth(label))));
  const none = marks(false, false);
  const items = request.items.map((item, index): OwnerListItem<string> => {
    const label = labels[index] ?? "", note = plainLabel(item.description ?? "");
    return {
      value: item.value,
      match: `${label} ${note}`,
      lines: (width, selected) => labelledLines(label, column, note, none, width, selected),
    };
  });
  return new Picker<string>({
    list: {
      title: [request.title, ...(request.description ? [request.description] : [])].join("\n"),
      placeholder: "type to filter", escape: "back",
      items: () => items,
      initial: rows => rows.findIndex(row => row.value === request.current),
      // Only the owner's navigation previews; opening the list or filtering it previews nothing, as in Pi's list.
      ...(request.preview ? { moved: (value: string | undefined) => { if (value !== undefined) request.preview?.(value); } } : {}),
    },
    choose: value => request.select(value),
    cancel: () => request.cancel(),
  }, host);
}
