import { fileURLToPath } from "node:url";
import pkg from "../package.json" with { type: "json" };
import { checkRuntime } from "./host/runtime.ts";
import { restrictEnvironment } from "./host/restrict.ts";

const usage = `Usage:
  corral-pi run --dir <run-dir> [--socket <path>] [--prompt-file <path>] [--corral <path>] [model flags]
  corral-pi chat --dir <chat-dir> [--socket <path>] [--corral <path>] [model flags]
  corral-pi build [--data-dir <path>] [--name <name> | --resume <id>] [--here] [--socket <path>] [--corral <path>] [model flags]
  corral-pi login [--provider <id> | --connection <id>] [--auth-dir <path>]
  corral-pi models [--provider <id>]
Model flags: --model [<provider>/]<id>, --provider <id>, --effort <level>, --aws-profile <name>, --aws-region <region>.
Sessions can also take --project <name> and --headless; non-interactive launches are headless by default.
`;

export async function main(argv = process.argv.slice(2), compiled = fileURLToPath(import.meta.url).startsWith("/$bunfs/")): Promise<void> {
  if (!compiled) checkRuntime();
  const [command, ...args] = argv;
  if (command === "--version") {
    process.stdout.write(`corral-pi pi ${pkg.dependencies["@earendil-works/pi-coding-agent"]} bun ${process.versions.bun}\n`);
    return;
  }
  if (!command || command === "--help" || args.includes("--help")) {
    process.stdout.write(usage);
    return;
  }
  restrictEnvironment(process.env);
  const assets = fileURLToPath(new URL("..", import.meta.url));
  if (compiled) await (await import("./host/compiled.ts")).prepareCompiled(command);
  switch (command) {
  // Defer SDK entry points until offline settings and compiled OAuth registration are installed.
    case "login": (await import("./auth/login.ts")).runLogin(args); return;
    case "models": (await import("./auth/models.ts")).runModels(args); return;
    case "build": await (await import("./host/build.ts")).buildCommand(args, assets, true); return;
    case "run": case "chat": {
      const { parseRunFlags } = await import("./host/config.ts");
      await (await import("./host/main.ts")).runHost(parseRunFlags(command, args, assets));
      return;
    }
    default: throw new Error(`unknown command ${command}; see corral-pi --help`);
  }
}

export function cliError(error: unknown): never {
  process.stderr.write(`corral-pi: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exit(2);
}

if (import.meta.main) await main().catch(cliError);
