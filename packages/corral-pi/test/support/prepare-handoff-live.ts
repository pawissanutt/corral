// Private assets for the real-service /handoff PTY scenario: the production assistant role and
// host, with only the offline fixture auth/delivery modules and an isolated Pi state directory.
import { cpSync } from "node:fs";
import { join } from "node:path";
import { buildFixtureInstall, PACKAGE_ROOT } from "./fixture-install.ts";

const work = process.argv[2];
if (!work) throw new Error("private fixture directory required");
const assets = buildFixtureInstall({ parent: work, omit: ["role"], isolatePiState: true });
cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
process.stdout.write(JSON.stringify({ assets: assets.dir }));
