// The identity of one nested Code Mode call (T-683). The runtime runs each child inside
// `ctx.executeTool` under this scope, so the tool Pi resolves (after the host's normal tool_call
// checks) can derive its request ID from `<codemode call>/<cell>.<n>` instead of Pi's own nested ID,
// which restarts once the parent call's record is taken and would collide for detached cells.
import { AsyncLocalStorage } from "node:async_hooks";

/** Mirrors corral-core's `CallParent`: the transcript call and kernel cell that made a child. */
export interface CodeParent {
  readonly tool_call_id: string;
  readonly cell: string;
  readonly call: string;
}

export interface CodeChild {
  readonly key: string;
  readonly parent: CodeParent;
  readonly tool: string;
  /** Set by the host's execution check when it refuses this child. */
  denied?: string;
}

const scope = new AsyncLocalStorage<CodeChild>();

export function childKey(parent: CodeParent): string {
  return `${parent.tool_call_id}/${parent.call}`;
}

export function runChild<T>(child: CodeChild, run: () => Promise<T>): Promise<T> {
  return scope.run(child, run);
}

/**
 * The child a nested execution of `tool` belongs to. A deeper call another tool makes carries a
 * different name or a transcript ID outside the parent, so it never inherits the identity.
 */
export function nestedChild(tool: string, toolCallId: string): CodeChild | undefined {
  const child = scope.getStore();
  return child && child.tool === tool && toolCallId.startsWith(`${child.parent.tool_call_id}/`) ? child : undefined;
}

/** The ID a receipted tool derives its request ID from. */
export function callKey(tool: string, toolCallId: string): string {
  return nestedChild(tool, toolCallId)?.key ?? toolCallId;
}
