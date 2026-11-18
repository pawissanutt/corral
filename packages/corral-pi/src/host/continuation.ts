import type { AssistantMessageEvent, StreamFunction } from "@earendil-works/pi-ai";
import type { StreamFn } from "@earendil-works/pi-agent-core";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { type ApiConnectionId, type ConnectionId } from "../auth/connections.ts";
import { type Spending } from "../capabilities/budget.ts";
import { messageFailure } from "./provider-failure.ts";

export function paidRoute(value: string): { connection: ApiConnectionId; model: string } {
  const match = /^(openai|anthropic)\/([^\s\0]+)$/.exec(value);
  if (!match) throw new Error("--paid-route requires openai/<model> or anthropic/<model>");
  return { connection: match[1] === "openai" ? "openai-api" : "anthropic-api", model: match[2]! };
}

/** Switch before emitting any output from a quota-refused request, keeping the same Pi turn. */
export function withPaidContinuation(stream: StreamFn, connection: ConnectionId, spending: Spending,
  route: string, paid: StreamFunction, notice: () => void): StreamFunction {
  return (model, context, options) => lazyStream(model, async () => {
    if (spending.activeRoute === route) return paid(model, context, options);
    const source = await stream(model, context, options);
    return (async function* () {
      let start: AssistantMessageEvent | undefined;
      let visible = false;
      for await (const event of source) {
        if (event.type === "start") { start = event; continue; }
        if (!visible && event.type === "error" && event.reason === "error" && !options?.signal?.aborted
            && messageFailure(event.error, connection).category === "subscription_quota") {
          if (spending.continueOn(route)) notice();
          yield* paid(model, context, options);
          return;
        }
        if (start) { yield start; start = undefined; }
        visible = true;
        yield event;
      }
    })();
  });
}
