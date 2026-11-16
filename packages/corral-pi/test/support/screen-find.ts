// Locate a label on the final frame of a captured PTY log: prints {"row","col"} (zero-based) or null.
import { readFileSync } from "node:fs";
import { ScreenModel } from "./screen.ts";

const [file, cols, rows, needle] = process.argv.slice(2);
if (!file || !cols || !rows || !needle) throw new Error("usage: screen-find <log> <cols> <rows> <needle>");
const raw = readFileSync(file, "latin1");
// The caller passes only output written since the last resize to this geometry.
const screen = new ScreenModel(Number(cols), Number(rows)).sync([Buffer.from(raw, "latin1").toString("utf8")]);
process.stdout.write(JSON.stringify(screen.find(needle) ?? null));
