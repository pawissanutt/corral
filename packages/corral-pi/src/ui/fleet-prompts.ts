// A session that is not a build session has no composer receiver and no docked dialog: giving a fleet child a
// message or an answer asks for the text in Pi's own input or select dialog, then goes through `FleetControl`.
// While the app can answer the child's question too, the dialog answers the host's card for that question
// instead of registering one of its own, and an answer from the app closes it.
import type { FleetControl } from "../build/types.ts";
import { optionId, type AnswerStatus } from "../host/remote/cards.ts";
import type { DialogResolution, ExistingDialog } from "../host/remote/dialogs.ts";
import type { ChildActionHost } from "./build/fleet-rows.ts";
import { helperText } from "./helper-chat.ts";
import { plainLabel } from "./panels-view.ts";

const OTHER = "Something else…";

/** The host's cards for children's questions, while the app can answer them too. */
export interface ChildCards {
  /** Watches child `id`'s question asked at `askedAt`; undefined when the session lists no such card. */
  watch(id: string, askedAt: number, settled: (result: DialogResolution) => void): ExistingDialog | undefined;
  /** Runs `open`, whose Pi dialog answers a watched card: it registers no card of its own and `bind` gets its close. */
  adopt<T>(bind: (close: () => void) => void, open: () => T): T;
}

export interface ChildPromptHost {
  readonly fleet: FleetControl;
  /** Pi's input dialog; resolves the typed text, undefined when dismissed. */
  input(title: string): Promise<string | undefined>;
  /** Pi's select dialog; resolves the chosen label. */
  select(title: string, options: readonly string[]): Promise<string | undefined>;
  notice(text: string, tone: "text" | "success" | "warning" | "error"): void;
  /** Present while a terminal is attached to a session with a dialog registry; absent: answers go straight to the fleet. */
  readonly cards?: ChildCards;
}

/** `point` (steer) and `question` (answer) for `childActions`, over dialogs instead of the build composer. */
export function childPrompts(host: ChildPromptHost): Pick<ChildActionHost, "point" | "question"> {
  const failed = (error: unknown) => host.notice(error instanceof Error ? error.message : String(error), "error");
  const label = (id: string) => plainLabel(helperText(id));
  return {
    point(id) {
      void host.input(`Message to ${label(id)}`).then(async text => {
        const message = text?.trim();
        if (!message) return;
        let delivered = false;
        try { delivered = await host.fleet.steer(id, message); } catch { delivered = false; }
        if (delivered) host.notice(`Steered ${label(id)}`, "success");
        else host.notice(`${label(id)} is not running · your message was not sent`, "warning");
      }).catch(failed);
    },
    question(id) {
      const asked = host.fleet.snapshot().children.find(child => child.id === id)?.question;
      if (!asked) { host.notice(`${label(id)} no longer waits for an answer`, "warning"); return; }
      const title = `${label(id)} asks: ${plainLabel(helperText(asked.question))}`, options = asked.options.map(option => plainLabel(helperText(option.label)));
      // The open prompt's close, while the app's answer should close it.
      let close: (() => void) | undefined;
      const cards = host.cards;
      const card = cards?.watch(id, asked.askedAt, result => {
        // Only an answer closes this prompt. A question put aside in the app (its "not now") keeps waiting, still watched.
        if (result.by?.source !== "remote" || result.kind !== "answered") return;
        close?.();
        host.notice("Answered from another device", "text");
      });
      const ask = async (open: () => Promise<string | undefined>) => {
        if (!card || !cards) return open();
        try { return await cards.adopt(dialog => { close = dialog; }, open); } finally { close = undefined; }
      };
      /** `text` is an offered option's label when `option` is its index, else the owner's own words. */
      const answer = async (text: string | undefined, option?: number) => {
        if (!text?.trim()) return;
        let delivered: boolean;
        if (card) {
          let status: AnswerStatus = "missing";
          try { status = await card.answer([option === undefined ? { options: [], text } : { options: [optionId(text, option)] }]); }
          catch { status = "missing"; }
          delivered = status === "delivered";
        } else delivered = host.fleet.answer(id, text);
        if (delivered) host.notice(`Answered ${label(id)}`, "success");
        else host.notice(`${label(id)} no longer waits for an answer`, "warning");
      };
      const prompt = async () => {
        if (!options.length) { await answer(await ask(() => host.input(title))); return; }
        const picked = await ask(() => host.select(title, [...options, OTHER]));
        if (picked === undefined) return;
        if (picked !== OTHER) { const index = options.indexOf(picked); await answer(asked.options[index]?.label, index); return; }
        await answer(await ask(() => host.input(title)));
      };
      void prompt().catch(failed).finally(() => card?.dispose());
    },
  };
}
