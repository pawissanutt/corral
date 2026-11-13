// Agent name colours. One native host per process, so the active appearance is module state, set next to
// `setPanelAppearance`. A hue is an index; it cycles through the appearance's eight colours.
import { AGENT_HUES, ansi, type Appearance } from "./appearance.ts";

let hues: readonly string[] = AGENT_HUES.dark.map(hex => ansi(hex));

export function setAgentHueAppearance(appearance: Appearance): void {
  hues = AGENT_HUES[appearance].map(hex => ansi(hex));
}
/** ANSI foreground for hue `hue`; any integer is valid and wraps around the palette. */
export function agentColor(hue: number): string {
  const size = hues.length, index = ((Math.trunc(hue) % size) + size) % size;
  return hues[index] ?? "";
}
