// Scope default cell colors to the app's alternate screen, never the terminal palette.
// Streaming lexer: only complete CSI SGR/mode sequences are interpreted; OSC/DCS/APC
// payloads (clipboard, links, images) remain opaque, even across write boundaries.
import { PANEL_TEXT } from "./panels-view.ts";

export const CANVAS_BG = "\x1b[48;2;15;16;18m";
export const CANVAS_STYLE = PANEL_TEXT + CANVAS_BG;
type State = "text" | "escape" | "escape-intermediate" | "csi" | "osc" | "string";

function defaultColors(body: string): string {
  if (!/^[\d;:]*$/.test(body)) return "";
  const groups = body.split(";");
  let foreground = false, background = false;
  for (let at = 0; at < groups.length; at++) {
    const group = groups[at] ?? "";
    const code = Number(group.split(":")[0]);
    if (code === 38 || code === 48 || code === 58) {
      // RGB/index values may contain 0/39/49: those are color channels, not resets.
      if (!group.includes(":")) {
        const format = groups[at + 1];
        if (format !== "2" && format !== "5") break;
        const count = format === "2" ? 4 : 2;
        if (at + count >= groups.length) break;
        at += count;
      }
      if (code === 38) foreground = false;
      if (code === 48) background = false;
    } else if (!group.includes(":")) {
      if (code === 0) { foreground = true; background = true; }
      else if (code === 39) foreground = true;
      else if (code === 49) background = true;
      else if (code >= 30 && code <= 37 || code >= 90 && code <= 97) foreground = false;
      else if (code >= 40 && code <= 47 || code >= 100 && code <= 107) background = false;
    }
  }
  return (foreground ? PANEL_TEXT : "") + (background ? CANVAS_BG : "");
}

export class PanelCanvas {
  private state: State = "text";
  private sequence: string | null = "";
  private stringEscape = false;
  private active = false;
  get atBoundary(): boolean { return this.state === "text"; }

  reset(): void {
    this.state = "text"; this.sequence = ""; this.stringEscape = false; this.active = false;
  }
  write(data: string): { data: string; screen: boolean | undefined; wrap: boolean | undefined } {
    let output = "", start = 0;
    let screen: boolean | undefined, wrap: boolean | undefined;
    for (let at = 0; at < data.length; at++) {
      const char = data[at];
      let suffix = "";
      if (char === "\x18" || char === "\x1a") {
        this.state = "text"; this.sequence = ""; this.stringEscape = false;
        continue;
      }
      switch (this.state) {
        case "text":
          if (char === "\x1b") this.state = "escape";
          break;
        case "escape":
          if (char === "[") { this.state = "csi"; this.sequence = ""; }
          else if (char === "]") { this.state = "osc"; this.stringEscape = false; }
          else if (char === "P" || char === "_" || char === "^" || char === "X") {
            this.state = "string"; this.stringEscape = false;
          } else if (char !== undefined && char >= " " && char <= "/") this.state = "escape-intermediate";
          else if (char !== undefined && char >= "0" && char <= "~") this.state = "text";
          break;
        case "escape-intermediate":
          if (char === "\x1b") this.state = "escape";
          else if (char !== undefined && char >= "0" && char <= "~") this.state = "text";
          break;
        case "osc":
        case "string":
          if (this.stringEscape && char === "\\" || char === "\x9c" || this.state === "osc" && char === "\x07") {
            this.state = "text"; this.stringEscape = false;
          } else this.stringEscape = char === "\x1b";
          break;
        case "csi":
          if (char === "\x1b") { this.state = "escape"; break; }
          if (this.sequence !== null) this.sequence = this.sequence.length < 256 ? this.sequence + char : null;
          if (char !== undefined && char >= "@" && char <= "~") {
            this.state = "text";
            if (this.sequence === "?1049h" || this.sequence === "?1049l") {
              screen = this.active = this.sequence === "?1049h";
              // Entry already saved the owner's attributes. Exit restores them; do not
              // recolor/reset the normal screen or send OSC palette setters.
              if (this.active) suffix = CANVAS_STYLE;
            } else if (this.sequence === "?7h" || this.sequence === "?7l") wrap = this.sequence === "?7h";
            else if (this.active && char === "m" && this.sequence !== null) suffix = defaultColors(this.sequence.slice(0, -1));
            this.sequence = "";
          }
          break;
      }
      if (suffix) { output += data.slice(start, at + 1) + suffix; start = at + 1; }
    }
    return { data: output + data.slice(start), screen, wrap };
  }
}
