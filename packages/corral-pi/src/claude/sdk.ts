// The one seam to `@anthropic-ai/claude-agent-sdk` (T-493): loaded lazily so an OpenAI host never
// imports it, and replaceable so tests drive the lane with a fake `query` and no network.
import { createRequire } from "node:module";
import { statSync } from "node:fs";

import type { Options, Query, SDKUserMessage, createSdkMcpServer } from "@anthropic-ai/claude-agent-sdk";

export interface SdkBoundary {
  query(input: { prompt: AsyncIterable<SDKUserMessage>; options: Options }): Query;
  createSdkMcpServer: typeof createSdkMcpServer;
}

let override: SdkBoundary | undefined;
let loaded: Promise<SdkBoundary> | undefined;

export function loadSdk(): Promise<SdkBoundary> {
  if (override) return Promise.resolve(override);
  loaded ??= import("@anthropic-ai/claude-agent-sdk").then((module) => ({
    query: (input) => module.query(input),
    createSdkMcpServer: module.createSdkMcpServer,
  })).catch((error: unknown) => {
    loaded = undefined;
    throw error;
  });
  return loaded;
}

/** Tests only: route every query to `boundary`; `undefined` restores the real SDK. */
export function overrideSdk(boundary: SdkBoundary | undefined): void {
  override = boundary;
}

export class ClaudeCodeUnavailable extends Error {
  readonly code = "claude_code_unavailable";
}

/** The platform packages the SDK resolves its native Claude Code binary from, in order. */
export function claudeCodeCandidates(platform: string, arch: string): string[] {
  const ext = platform === "win32" ? ".exe" : "";
  if (platform === "linux") {
    return [`@anthropic-ai/claude-agent-sdk-linux-${arch}/claude`, `@anthropic-ai/claude-agent-sdk-linux-${arch}-musl/claude`];
  }
  return [`@anthropic-ai/claude-agent-sdk-${platform}-${arch}/claude${ext}`];
}

export interface ExecutableDeps {
  readonly platform: string;
  readonly arch: string;
  resolve(specifier: string): string;
  isFile(path: string): boolean;
}

function defaultDeps(): ExecutableDeps {
  const require = createRequire(import.meta.url);
  return {
    platform: process.platform, arch: process.arch,
    resolve: (specifier) => require.resolve(specifier),
    isFile: (path) => {
      try {
        return statSync(path).isFile();
      } catch {
        return false;
      }
    },
  };
}

/**
 * The Claude Code binary the SDK's platform package installed next to it. No PATH lookup and no
 * download: a missing package is refused with a stable code naming what to install.
 */
export function resolveClaudeCode(deps: ExecutableDeps = defaultDeps()): string {
  const candidates = claudeCodeCandidates(deps.platform, deps.arch);
  for (const candidate of candidates) {
    let path: string;
    try {
      path = deps.resolve(candidate);
    } catch {
      continue;
    }
    if (deps.isFile(path)) return path;
  }
  const wanted = candidates[0]?.slice(0, candidates[0].lastIndexOf("/")) ?? "@anthropic-ai/claude-agent-sdk";
  throw new ClaudeCodeUnavailable(`claude_code_unavailable: no Claude Code executable for ${deps.platform}-${deps.arch}; ` +
    `install ${wanted} (reinstall the host with \`corral agent install\` on a supported platform)`);
}
