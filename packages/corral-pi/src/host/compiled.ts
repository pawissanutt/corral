// The entry point `bun build --compile` turns into the release `corral-pi-host` executable.
// It restricts the environment before any Pi module loads, then:
//   --version                 prints the pinned Pi and the embedded Bun versions;
//   --write-install <dir>     writes compat.json and the role assets beside the installed binary;
//   login ...                 runs `corral agent login`'s OAuth flow (src/auth/login.ts);
//   models --provider <id>... prints Pi's offline model catalog for harness detection
//                             (src/auth/models.ts);
//   bare ...                  runs `corral pi --bare` (src/host/bare.ts) with only the bundled
//                             auth: no role or delivery module is imported;
//   anything else             runs the host (src/host/main.ts) with the bundled role, delivery and
//                             auth modules instead of the fixed-path loader.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import compat from "../../compat.json" with { type: "json" };
import pmContext from "../../assets/pm-context.md" with { type: "text" };
import reviewerContract from "../../assets/reviewer-contract.md" with { type: "text" };
import workerContract from "../../assets/worker-contract.md" with { type: "text" };
import workerSkill from "../roles/worker/skills/corral-worker/SKILL.md" with { type: "text" };
import { restrictEnvironment } from "./restrict.ts";

export const INSTALL_FILES: Readonly<Record<string, string>> = {
  "compat.json": `${JSON.stringify(compat, null, 2)}\n`,
  "assets/pm-context.md": pmContext,
  "assets/reviewer-contract.md": reviewerContract,
  "assets/worker-contract.md": workerContract,
  "src/roles/worker/skills/corral-worker/SKILL.md": workerSkill,
};

function writeInstall(dir: string): void {
  for (const [relative, text] of Object.entries(INSTALL_FILES)) {
    const path = join(dir, relative);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text, { mode: 0o644 });
  }
}

async function installBundle(): Promise<void> {
  const [{ setBundle }, worker, reviewer, pm, delivery, auth] = await Promise.all([
    import("./bundle.ts"), import("../roles/worker/index.ts"), import("../roles/reviewer/index.ts"),
    import("../roles/pm/index.ts"), import("../delivery/index.ts"), import("../auth/index.ts"),
  ]);
  setBundle({
    roles: { worker: (root) => worker.workerRole(root), reviewer: (root) => reviewer.reviewerRole(root),
      pm: (root) => pm.pmRole(root) },
    delivery: delivery.default, auth: auth.default,
  });
}

async function runHost(): Promise<void> {
  await installBundle();
  await import("./main.ts");
}

restrictEnvironment(process.env);
const [command, operand] = process.argv.slice(2);
if (command === "--version") {
  process.stdout.write(`corral-pi-host pi ${compat.pi.version} bun ${process.versions.bun}\n`);
} else if (command === "--write-install") {
  if (!operand) {
    process.stderr.write("corral-pi-host: --write-install needs a directory\n");
    process.exit(2);
  }
  writeInstall(operand);
} else if (command === "login") {
  (await import("../auth/login.ts")).runLogin(process.argv);
} else if (command === "models") {
  (await import("../auth/models.ts")).runModels(process.argv);
} else if (command === "bare") {
  const [{ setBundle }, auth] = await Promise.all([import("./bundle.ts"), import("../auth/index.ts")]);
  setBundle({ auth: auth.default });
  // The installation directory is the one this executable was installed into.
  await (await import("./bare.ts")).bareCommand(process.argv.slice(3), dirname(process.execPath));
} else {
  await runHost();
}
