import { BridgeRefusal, SESSION_PART_MAX } from "../../bridge/protocol.ts";
import { EXPORT_COMMAND } from "../export.ts";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, fitRows } from "./state.ts";

interface SlashCommand { name: string; description: string; source: "host" | "extension" | "prompt" | "skill"; args_hint?: string }
const HOST_COMMANDS: readonly SlashCommand[] = [
  { name: "model", description: "Pick a model and effort", source: "host" },
  { name: "compact", description: "Summarize older context", source: "host", args_hint: "[instructions]" },
  { name: "actions", description: "Open the session action palette", source: "host" },
  { name: "questions", description: "Open the question dock", source: "host" },
  { name: "panels", description: "Open a session view", source: "host", args_hint: "<view>" },
  { name: "thinking", description: "Toggle thinking in the transcript", source: "host" },
];
const HIDDEN: Readonly<Record<string, true>> = {
  settings: true, login: true, logout: true, new: true, fork: true, resume: true, switch: true,
  import: true, tree: true, reload: true, quit: true, exit: true,
};

export function commandsPart(registry: RemoteRegistry, input: {
  extensions: readonly { invocationName: string; description?: string }[];
  prompts: readonly { name: string; description?: string }[];
  skills: readonly { name: string; description?: string }[];
  rewind: boolean;
}) {
  const commands = [...HOST_COMMANDS];
  if (registry.get("exit_open")) commands.push({ name: "finish", description: "Choose how to finish the build", source: "host" });
  if (registry.get("board_refresh")) commands.push({ name: "board", description: "Refresh the build board", source: "host" });
  if (registry.get("export")) commands.push({ name: EXPORT_COMMAND, description: "Attach an offline HTML session export", source: "host" });
  if (input.rewind) commands.push({ name: "rewind", description: "Restore a recorded turn", source: "host", args_hint: "[--list]" });
  if (registry.get("plan")) commands.push({ name: "todo", description: "Show the session plan", source: "host" },
    { name: "plan", description: "Show the session plan", source: "host" });
  const seen = new Set(commands.map(command => command.name));
  function add(name: string, description: string | undefined, source: SlashCommand["source"]) {
    if (seen.has(name) || Object.hasOwn(HIDDEN, name) || !name || Buffer.byteLength(name) > 256) return;
    seen.add(name);
    commands.push({ name, description: clipText(description ?? "", 512), source });
  }
  for (const command of input.extensions) add(command.invocationName, command.description, "extension");
  for (const prompt of input.prompts) add(prompt.name, prompt.description, "prompt");
  for (const skill of input.skills) add(`skill:${skill.name}`, skill.description, "skill");
  return { commands: fitRows(commands, 0, 200, SESSION_PART_MAX - 32) };
}

/** Commands with a host action use the same preflight when typed instead of picked. */
export function typedSessionAction(registry: RemoteRegistry, text: string) {
  const command = /^\/(finish|board|compact|corral-export)(?:[ \t]+([\s\S]*))?$/.exec(text);
  if (!command) return;
  const name = command[1], args = command[2]?.trim();
  const method = name === "finish" ? "exit_open" : name === "board" ? "board_refresh" : name === "compact" ? "compact" : "export";
  const run = registry.get(method);
  if (!run) return;
  return Promise.resolve().then(() => {
    if (args && method !== "compact") throw new BridgeRefusal("bad_input", "this command does not take arguments");
    return run(method === "compact" && args ? { instructions: args } : {});
  });
}

export function registerCommands(registry: RemoteRegistry, source: {
  list(): object; ready(): boolean; compact(instructions?: string): Promise<{ tokensBefore: number }>;
  export?: () => Promise<{ text: string; committed: boolean }>;
}): void {
  registry.register({ name: "commands", parse: () => undefined, run: () => source.list() });
  let compacting = false;
  registry.register({ name: "compact", parse: ({ instructions }) => {
    if (instructions !== undefined && (typeof instructions !== "string" || Buffer.byteLength(instructions) > 16 * 1024)) {
      throw new BridgeRefusal("bad_input", "invalid compaction instructions");
    }
    return instructions;
  }, run: async instructions => {
    if (!source.ready() || compacting) throw new BridgeRefusal("compact_unavailable", "Wait for the current session action to finish");
    compacting = true;
    try {
      const result = await source.compact(instructions);
      return { text: `Compacted session context (${result.tokensBefore} tokens before compaction)` };
    } finally { compacting = false; }
  } });
  const exportSession = source.export;
  if (exportSession) registry.register({ name: "export", parse: () => undefined, run: async () => {
    if (!source.ready()) throw new BridgeRefusal("export_unavailable", "Wait for the current session action to finish");
    const { text, committed } = await exportSession();
    return { text: clipText(text, 4096), committed };
  } });
}
