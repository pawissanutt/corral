// Tool mapping for the Claude Agent SDK lane (T-493), ported from senpi's anthropic-subscription
// tools.js (MIT). Claude Code plans tool calls; Pi executes them. Pi's built-in tools map to Claude
// Code's own Read/Write/Edit/Bash/Grep/Glob, every other Pi tool is exposed through an in-process MCP
// server, and a PreToolUse hook plus `canUseTool` deny every execution inside Claude Code.
import type { Tool } from "@earendil-works/pi-ai";

export const SDK_TO_PI_TOOL_NAME: Readonly<Record<string, string>> = {
  read: "read", write: "write", edit: "edit", bash: "bash", grep: "grep", glob: "find",
};

export const PI_TO_SDK_TOOL_NAME: Readonly<Record<string, string>> = {
  read: "Read", write: "Write", edit: "Edit", bash: "Bash", grep: "Grep", find: "Glob", glob: "Glob",
};

export const BUILTIN_SDK_TOOLS = ["Read", "Write", "Edit", "Bash", "Grep", "Glob"] as const;

export const TOOL_EXECUTION_DENIED_MESSAGE = "The corral Pi host executes this tool and returns its result " +
  "as the next user message. Wait for that result; this denial is not a failure.";

export const CUSTOM_TOOLS_MCP_SERVER_NAME = "custom-tools";
export const CUSTOM_TOOLS_MCP_PREFIX = `mcp__${CUSTOM_TOOLS_MCP_SERVER_NAME}__`;
export const HOST_CAPTURED_SDK_TOOL_MATCHER = "Bash|Write|Edit|Read|Grep|Glob|mcp__custom-tools__.*";

/** What the PreToolUse hook returns for every captured tool: deny, and stop Claude Code's loop. */
export const PRE_TOOL_USE_DENIAL = {
  continue: false,
  hookSpecificOutput: {
    hookEventName: "PreToolUse" as const,
    permissionDecision: "deny" as const,
    permissionDecisionReason: TOOL_EXECUTION_DENIED_MESSAGE,
  },
};

export const HOST_TOOL_DENIAL_HOOKS = {
  PreToolUse: [{ matcher: HOST_CAPTURED_SDK_TOOL_MATCHER, hooks: [async () => PRE_TOOL_USE_DENIAL] }],
};

export const canUseTool = async () => ({ behavior: "deny" as const, message: TOOL_EXECUTION_DENIED_MESSAGE });

function pascalCase(value: string): string {
  return value.split(/[^a-zA-Z0-9]+/).filter(Boolean).map((part) => `${part[0]?.toUpperCase() ?? ""}${part.slice(1)}`)
    .join("");
}

export function mapPiToolNameToSdk(name: string, customToolNameToSdk?: ReadonlyMap<string, string>): string {
  const normalized = name.toLowerCase();
  return customToolNameToSdk?.get(name) ?? customToolNameToSdk?.get(normalized) ?? PI_TO_SDK_TOOL_NAME[normalized] ??
    pascalCase(name);
}

export function mapSdkToolNameToPi(name: string, customToolNameToPi?: ReadonlyMap<string, string>): string {
  const normalized = name.toLowerCase();
  return SDK_TO_PI_TOOL_NAME[normalized] ?? customToolNameToPi?.get(name) ?? customToolNameToPi?.get(normalized) ??
    (normalized.startsWith(CUSTOM_TOOLS_MCP_PREFIX) ? name.slice(CUSTOM_TOOLS_MCP_PREFIX.length) : name);
}

type Args = Record<string, unknown>;

function defined(value: Args): Args {
  return Object.fromEntries(Object.entries(value).filter(([, entry]) => entry !== undefined));
}

/** Translates Claude Code's built-in tool inputs into the arguments of Pi's tool `piName`. */
export function mapToolArgs(piName: string, args: unknown): Args {
  const input = (args !== null && typeof args === "object" && !Array.isArray(args) ? args : {}) as Args;
  switch (piName.toLowerCase()) {
    case "read":
      return defined({ path: input.file_path ?? input.path, offset: input.offset, limit: input.limit });
    case "write":
      return defined({ path: input.file_path ?? input.path, content: input.content });
    case "edit":
      return defined({
        path: input.file_path ?? input.path,
        edits: Array.isArray(input.edits) ? input.edits : [defined({
          oldText: input.old_string ?? input.oldText ?? input.old_text,
          newText: input.new_string ?? input.newText ?? input.new_text,
        })],
      });
    case "bash":
      return defined({
        command: input.command,
        timeout: typeof input.timeout === "number" ? input.timeout / 1_000 : input.timeout,
      });
    case "grep":
      return defined({
        pattern: input.pattern, path: input.path, glob: input.glob,
        ignoreCase: input["-i"] ?? input.ignoreCase, context: input.context ?? input["-C"],
        limit: input.head_limit ?? input.limit,
      });
    case "find":
    case "glob":
      return defined({ pattern: input.pattern, path: input.path, limit: input.limit });
    default:
      return { ...input };
  }
}

export interface ResolvedSdkTools {
  readonly sdkTools: string[];
  readonly customTools: Tool[];
  readonly customToolNameToSdk: Map<string, string>;
  readonly customToolNameToPi: Map<string, string>;
}

/** Splits Pi's active tools into Claude Code built-ins and custom MCP tools. */
export function resolveSdkTools(tools: readonly Tool[]): ResolvedSdkTools {
  const sdkTools = new Set<string>();
  const customTools: Tool[] = [];
  const customToolNameToSdk = new Map<string, string>();
  const customToolNameToPi = new Map<string, string>();
  for (const tool of tools) {
    const normalized = tool.name.toLowerCase();
    const sdkName = PI_TO_SDK_TOOL_NAME[normalized];
    if (sdkName) {
      sdkTools.add(sdkName);
      continue;
    }
    const customSdkName = `${CUSTOM_TOOLS_MCP_PREFIX}${tool.name}`;
    customTools.push(tool);
    customToolNameToSdk.set(tool.name, customSdkName);
    customToolNameToSdk.set(normalized, customSdkName);
    customToolNameToPi.set(customSdkName, tool.name);
    customToolNameToPi.set(customSdkName.toLowerCase(), tool.name);
  }
  return { sdkTools: [...sdkTools], customTools, customToolNameToSdk, customToolNameToPi };
}
