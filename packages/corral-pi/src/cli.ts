import { codeWorkerFlag, receiveCodeBootstrap, superviseCode, type CodeBootstrap } from "./host/code-handoff.ts";
import { fileURLToPath } from "node:url";
import pkg from "../package.json" with { type: "json" };
import { checkRuntime } from "./host/runtime.ts";
import { restrictEnvironment } from "./host/restrict.ts";

const usage = `Usage:
  corral-pi run --dir <run-dir> [--socket <path>] [--prompt-file <path>] [--corral <path>] [model flags]
  corral-pi chat [--new | --dir <chat-dir>] [--plan] [--socket <path>] [--corral <path>] [model flags]
  corral-pi build [--data-dir <path>] [--id <id>] [--name <name>] [--profile <name>] [--here] [--plan] [--socket <path>] [--corral <path>] [--headless] [model flags]
  corral-pi build --resume <id> [--data-dir <path>] [--socket <path>] [--corral <path>] [--headless] [--no-fallback]
  corral-pi code [--data-dir <path>] [--id <id>] [--name <name>] [--profile <name>] [--plan] [--socket <path>] [--corral <path>] [--headless] [model flags]
  corral-pi code --resume <id> [--data-dir <path>] [--socket <path>] [--corral <path>] [--headless] [--no-fallback]
  corral-pi build --prune [--session <id> [--json]] [--dry-run] [--data-dir <path>]
  corral-pi login [--provider <id>] [--auth-dir <path>]
  corral-pi models [--provider <id>]
Model flags: --model [<provider>/]<id>, --provider <id>, --effort <level>, --aws-profile <name>, --aws-region <region>.
New terminal chat and Build sessions use a saved startup model or ask you to choose one.\nFor new headless sessions without an available default, pass --model <provider/model>.
Fallback: configure [fallback.providers] in .corral-next/pi.toml, repeat --fallback <provider>/<model>, or pass --no-fallback.
Sessions can also take --project <name> and --headless; non-interactive launches are headless by default.
Code explicitly trusts the parent agent to work directly in the current directory with your permissions, without creating a worktree.
--plan starts chat, Build or Code in plan mode: the agent plans with you and waits for your approval before changing anything.
`;

export async function main(argv = process.argv.slice(2), compiled = fileURLToPath(import.meta.url).startsWith("/$bunfs/")): Promise<void> {
  if (!compiled) checkRuntime();
  let [command, ...args] = argv;
  let codeBootstrap: CodeBootstrap | undefined;
  if (command === codeWorkerFlag) {
    codeBootstrap = await receiveCodeBootstrap();
    command = "code";
  }
  if (command === "--version") {
    process.stdout.write(`corral-pi pi ${pkg.dependencies["@earendil-works/pi-coding-agent"]} bun ${process.versions.bun}\n`);
    return;
  }
  if (!command || command === "--help" || args.includes("--help")) {
    process.stdout.write(usage);
    return;
  }
  if (command === "code" && !codeBootstrap && process.stdin.isTTY && !args.some(arg => /^--headless(?:=|$)/.test(arg))) {
    const executable = compiled ? [process.execPath] : [process.execPath, fileURLToPath(import.meta.url)];
    process.exitCode = await superviseCode(executable, { cwd: process.cwd(), args });
    return;
  }
  restrictEnvironment(process.env);
  const assets = fileURLToPath(new URL("..", import.meta.url));
  if (compiled) await (await import("./host/compiled.ts")).prepareCompiled(command === "code" ? "build" : command);
  switch (command) {
  // Defer SDK entry points until offline settings and compiled OAuth registration are installed.
    case "--check-code-kernel": await (await import("./host/code-kernel-check.ts")).checkCodeKernel(); return;
    case "--write-install": {
      if (!compiled || !args[0]) throw new Error("--write-install needs a compiled executable and a directory");
      (await import("./host/compiled.ts")).writeInstall(args[0]);
      return;
    }
    case "login": (await import("./auth/login.ts")).runLogin(args); return;
    case "models": (await import("./auth/models.ts")).runModels(args); return;
    case "code": case "build": await (await import("./host/build.ts")).buildCommand(args, assets, command, codeBootstrap); return;
    case "run": case "chat": {
      const { parseRunFlags } = await import("./host/config.ts");
      const config = parseRunFlags(command, args, assets);
      await (await import("./host/main.ts")).runHost(config);
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
