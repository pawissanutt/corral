// A session that is not a build session has no composer receiver and no docked dialog: giving a fleet child a
// message or an answer asks for the text in Pi's own input or select dialog, then goes through `FleetControl`.
import type { FleetControl } from "../build/types.ts";
import type { ChildActionHost } from "./build/fleet-rows.ts";
import { helperText } from "./helper-chat.ts";
import { plainLabel } from "./panels-view.ts";

const OTHER = "Something else…";

export interface ChildPromptHost {
  readonly fleet: FleetControl;
  /** Pi's input dialog; resolves the typed text, undefined when dismissed. */
  input(title: string): Promise<string | undefined>;
  /** Pi's select dialog; resolves the chosen label. */
  select(title: string, options: readonly string[]): Promise<string | undefined>;
  notice(text: string, tone: "text" | "success" | "warning" | "error"): void;
}

/** `point` (steer) and `question` (answer) for `childActions`, over dialogs instead of the build composer. */
export function childPrompts(host: ChildPromptHost): Pick<ChildActionHost, "point" | "question"> {
  const failed = (error: unknown) => host.notice(error instanceof Error ? error.message : String(error), "error");
  const label = (id: string) => plainLabel(helperText(id));
  const answer = (id: string, text: string | undefined) => {
    if (!text?.trim()) return;
    if (host.fleet.answer(id, text)) host.notice(`Answered ${label(id)}`, "success");
    else host.notice(`${label(id)} no longer waits for an answer`, "warning");
  };
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
      if (!options.length) { void host.input(title).then(text => answer(id, text)).catch(failed); return; }
      void host.select(title, [...options, OTHER]).then(async picked => {
        if (picked === undefined) return;
        if (picked !== OTHER) { answer(id, asked.options[options.indexOf(picked)]?.label); return; }
        answer(id, await host.input(title));
      }).catch(failed);
    },
  };
}
