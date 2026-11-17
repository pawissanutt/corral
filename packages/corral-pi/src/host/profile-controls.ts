// /profiles and the next-profile key: the owner sees the agent profiles this session can run and picks one. The
// backend's controller owns what a pick does: it may restart the session under supervision, or refuse (a run is
// streaming, the switch would reset a paid lineage, a policy needs a new session). This module asks through Pi's
// own select (the owner decision card in rich panels), reports the outcome as a notice and never switches to a
// profile the controller marks unavailable. A switch that opens a new session (and, for an API profile, a new
// allowance) is confirmed first, from the picker and the key alike; a declined confirmation changes nothing. It
// approves no other spending and sends no model request.
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { AutocompleteItem, KeyId } from "@earendil-works/pi-tui";
import { CONNECTIONS, type ConnectionId } from "../auth/connections.ts";
import { freeKey } from "../ui/keymap.ts";

export const PROFILES_COMMAND = "profiles";
/**
 * The next-profile key ("rotate"). Not Ctrl+P, Pi's next model; not Alt+P, which Pi's Windows and WSL table binds to
 * the previous model; not Alt+O, which legacy terminals send as the start of an SS3 key.
 */
export const PROFILE_KEY: KeyId = "alt+r";

/** One profile as the backend offers it to this session. */
export interface ProfileChoice {
  /** The profile's exact config name (`pi-opus-api`); `select` takes it. */
  readonly name: string;
  /** What the owner reads beside the name; not shown when it repeats the name. */
  readonly label: string;
  /** The exact model it runs, `provider/model`. */
  readonly model: string;
  /** The connection its requests use, which says how they are paid: subscription, metered API or local. */
  readonly connection: ConnectionId;
  /** False when a switch to it would be refused now; `reason` says why. */
  readonly available: boolean;
  readonly reason?: string;
  /** Switching to it opens a new session and conversation instead of continuing this one; the owner confirms first. */
  readonly newSession?: boolean;
  /** The USD allowance, in micro-dollars, that the new session gets for its own API requests (not a shared budget). */
  readonly allowanceMicros?: number;
}

/** What the backend supplies: the profiles, the current one, and the switch itself. */
export interface ProfileController {
  /** Every profile the owner may see for this session, in the order offered, unavailable ones included. */
  list(): Promise<readonly ProfileChoice[]>;
  /** The profile this session runs, or undefined while it runs its mode's default. */
  current(): string | undefined;
  /**
   * Why no switch can start right now (a streaming run, a compaction), or undefined when one can. Optional: without it
   * `select` alone refuses.
   */
  blocked?(): string | undefined;
  /**
   * Switches to `name`, which may restart the session under supervision. Resolves once the switch is accepted;
   * rejects with an owner-readable reason when it is refused (a paid lineage it would reset, a policy that needs a
   * new session or a confirmation), and then the current profile is unchanged.
   */
  select(name: string): Promise<void>;
}

export interface ProfileControlOptions {
  /** The next-profile key; `null` registers none. PROFILE_KEY when absent. Never one the owner bound elsewhere. */
  readonly key?: KeyId | null;
}

const plain = (value: string) => value.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").trim();
const reason = (error: unknown) => plain(error instanceof Error ? error.message : String(error)) || "the switch was refused";

/** The key installProfileControls registers for `options`: undefined for none, or when the owner bound it to something else. */
export function profileKey(options: ProfileControlOptions = {}): KeyId | undefined {
  return options.key === null ? undefined : freeKey(options.key ?? PROFILE_KEY);
}

/** How a connection's requests are paid, as a row says it: `subscription`, `API · metered` or `local`. */
export function billingWord(connection: ConnectionId): string {
  const billing = CONNECTIONS[connection].billing;
  return billing === "metered" ? "API · metered" : billing;
}

/** A profile as one select row: name, label, exact model, billing, then `current` or `unavailable: <reason>`. */
export function profileRow(choice: ProfileChoice, current: string | undefined): string {
  const parts = [plain(choice.name)];
  if (plain(choice.label) && plain(choice.label) !== plain(choice.name)) parts.push(plain(choice.label));
  parts.push(plain(choice.model), billingWord(choice.connection));
  if (choice.name === current) parts.push("current");
  if (!choice.available) parts.push(`unavailable: ${plain(choice.reason ?? "") || "no reason given"}`);
  else if (choice.newSession === true) parts.push("new conversation");
  return parts.join(" · ");
}

/** The select's title: the question, then what the session runs now. */
export function profileTitle(list: readonly ProfileChoice[], current: string | undefined): string {
  const now = list.find(choice => choice.name === current);
  const line = current === undefined ? "Now: this mode's default profile"
    : now ? `Now: ${plain(now.name)} · ${plain(now.model)} · ${billingWord(now.connection)}` : `Now: ${plain(current)}`;
  return `Choose this session's profile\n${line}`;
}

/** The next available profile after `current` in list order, wrapping; from the default, the first available one. */
export function nextProfile(list: readonly ProfileChoice[], current: string | undefined): ProfileChoice | undefined {
  const from = list.findIndex(choice => choice.name === current);
  for (let step = 1; step <= list.length; step++) {
    const choice = list[(from + step) % list.length];
    if (choice?.available && choice.name !== current) return choice;
  }
  return undefined;
}

/** The confirmation a switch to a new session asks: its title and message (model, billing, conversation, allowance). */
export function newSessionQuestion(choice: ProfileChoice): { readonly title: string; readonly message: string } {
  const lines = [`${plain(choice.model)} · ${billingWord(choice.connection)}`,
    "This opens a new session with a new conversation; the current conversation is not carried into it."];
  if (choice.allowanceMicros !== undefined) {
    lines.push(`Its API allowance is USD ${(choice.allowanceMicros / 1_000_000).toFixed(2)} for that new session alone, not a shared or daily budget.`);
  }
  return { title: `Open a new conversation with ${plain(choice.name)}?`, message: lines.join("\n") };
}

/**
 * Registers `/profiles [name]` and the next-profile key over `controller`. `/profiles` shows the current profile and
 * one row per profile; `/profiles <name>` switches to exactly that one. A cancelled select changes nothing; an
 * unavailable, blocked or refused switch is a notice and the profile stays as it was. A switch to a `newSession`
 * profile, by any route, is confirmed first; declining it changes nothing. Returns the key it registered.
 */
export function installProfileControls(pi: ExtensionAPI, controller: ProfileController,
  options: ProfileControlOptions = {}): { readonly key: KeyId | undefined } {
  let switching = false;

  const read = async (ctx: ExtensionContext): Promise<readonly ProfileChoice[] | undefined> => {
    try { return await controller.list(); } catch (error) {
      ctx.ui.notify(`Profiles could not be read · ${reason(error)}`, "error");
      return undefined;
    }
  };

  const choose = async (ctx: ExtensionContext, choice: ProfileChoice): Promise<void> => {
    const name = plain(choice.name);
    if (!choice.available) {
      ctx.ui.notify(`Profile unchanged · ${name} is unavailable: ${plain(choice.reason ?? "") || "no reason given"}`, "warning");
      return;
    }
    if (choice.name === controller.current()) { ctx.ui.notify(`Already on ${name}`, "info"); return; }
    if (switching) { ctx.ui.notify("Profile unchanged · a profile switch is already under way", "warning"); return; }
    const refusal = () => {
      const blocked = controller.blocked?.();
      if (blocked !== undefined) ctx.ui.notify(`Profile unchanged · ${plain(blocked)}`, "warning");
      return blocked !== undefined;
    };
    if (refusal()) return;
    // Held across the confirmation too, so a second key press cannot open another one.
    switching = true;
    try {
      if (choice.newSession === true) {
        const question = newSessionQuestion(choice);
        // Esc, a timeout or an abort: nothing changes and nothing is said.
        if (!await ctx.ui.confirm(question.title, question.message)) return;
        if (refusal()) return;
      }
      await controller.select(choice.name);
      ctx.ui.notify(`Profile → ${name} · ${plain(choice.model)} · ${billingWord(choice.connection)}`, "info");
    } catch (error) {
      ctx.ui.notify(`Profile unchanged · ${reason(error)}`, "error");
    } finally {
      switching = false;
    }
  };

  pi.registerCommand(PROFILES_COMMAND, {
    description: "Show this session's profile and choose another; /profiles <name> switches to it",
    getArgumentCompletions: async (prefix): Promise<AutocompleteItem[] | null> => {
      let list: readonly ProfileChoice[];
      try { list = await controller.list(); } catch { return null; }
      const typed = prefix.trim();
      return list.filter(choice => choice.name.startsWith(typed)).map(choice => ({
        value: choice.name, label: choice.name,
        description: `${plain(choice.model)} · ${billingWord(choice.connection)}${choice.available ? "" : " · unavailable"}`,
      }));
    },
    handler: async (args, ctx) => {
      const list = await read(ctx);
      if (!list) return;
      const name = args.trim();
      if (name) {
        const choice = list.find(item => item.name === name);
        if (choice) await choose(ctx, choice);
        else ctx.ui.notify(`Profile unchanged · no profile is named ${plain(name)}; /profiles lists them`, "warning");
        return;
      }
      if (!list.length) { ctx.ui.notify("No profiles are offered to this session", "info"); return; }
      const current = controller.current();
      const rows = list.map(choice => profileRow(choice, current));
      const picked = await ctx.ui.select(profileTitle(list, current), rows);
      // Esc, a timeout or an abort: nothing changes.
      if (picked === undefined) return;
      const choice = list[rows.indexOf(picked)];
      if (choice) await choose(ctx, choice);
    },
  });

  const key = profileKey(options);
  if (key) {
    pi.registerShortcut(key, {
      description: "Switch to the next available profile",
      handler: async ctx => {
        if (switching) { ctx.ui.notify("Profile unchanged · a profile switch is already under way", "warning"); return; }
        const list = await read(ctx);
        if (!list) return;
        const next = nextProfile(list, controller.current());
        if (next) { await choose(ctx, next); return; }
        ctx.ui.notify(list.some(choice => choice.available) ? "No other profile is available" : "No profile is available now",
          "warning");
      },
    });
  }
  return { key };
}
