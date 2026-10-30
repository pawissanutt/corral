// The assistant's fixed shell refusals (T-544): no input into a herdr pane and no direct write
// under `.corral/`. They stop an honest model's accidents, not an adversary. Presets, capability
// settings and configured bash rules can neither disable nor override them. The board changes
// only through corral commands, which audit and attribute each change.
import { pieces } from "../../capabilities/bash-rules.ts";

const PANE_INPUT = /(?:^|[\s/])herdr(?:\s+\S+)*?\s+(?:pane\s+(?:send-text|send-keys|run)|agent\s+prompt)(?:\s|$)/;
/** Each redirection target of a piece (`>`, `>>`, `>|`, with an optional descriptor). */
const REDIRECT = /(?:^|[^<>])>{1,2}\|?\s*([^\s<>|&;]+)/g;
const WRAPPERS = new Set(["sudo", "env", "command", "nice", "nohup", "time", "exec"]);
const WRITERS = new Set([
  "rm", "rmdir", "mv", "cp", "tee", "touch", "mkdir", "ln", "install", "truncate", "chmod", "chown", "chgrp",
  "rsync", "unlink", "shred", "dd",
]);
const IN_PLACE = new Set(["sed", "perl"]);

function unquote(token: string): string {
  return token.replace(/^["']+|["']+$/g, "");
}

/** A path with a `.corral` component, the workspace store's directory. */
function inStore(token: string): boolean {
  return /(?:^|\/)\.corral(?:\/|$)/.test(unquote(token).replace(/^of=/, ""));
}

function writesStore(piece: string): boolean {
  for (const match of piece.matchAll(REDIRECT)) if (inStore(match[1] ?? "")) return true;
  const words = piece.split(/\s+/).filter((word) => word.length > 0);
  let index = 0;
  while (index < words.length && (WRAPPERS.has(words[index] ?? "") || /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[index] ?? ""))) {
    index += 1;
  }
  const name = (words[index] ?? "").split("/").pop() ?? "";
  const operands = words.slice(index + 1);
  if (WRITERS.has(name)) return operands.some(inStore);
  if (IN_PLACE.has(name)) return operands.some((word) => /^-i|^--in-place/.test(word)) && operands.some(inStore);
  return false;
}

/** Why the assistant may not run `command`, or undefined when it may. Nested shells count. */
export function assistantShellRefusal(command: string): string | undefined {
  // The whole line too: splitting at `|` would separate a `>|` redirection from its target.
  for (const piece of [command.replace(/\s+/g, " ").trim(), ...pieces(command)]) {
    if (PANE_INPUT.test(piece)) {
      return `assistant rule: no input into a herdr pane (${JSON.stringify(piece)}); nothing ran. `
        + "An assistant never types into another pane: tell the owner, or hand the request to the PM.";
    }
    if (writesStore(piece)) {
      return `assistant rule: no direct write under .corral/ (${JSON.stringify(piece)}); nothing ran. `
        + "Change the board through corral commands, which record who changed what.";
    }
  }
  return undefined;
}
