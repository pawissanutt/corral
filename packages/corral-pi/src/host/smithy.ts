// The Bedrock host's network boundary (T-504, guard option a). pi-ai's Bedrock client, and the
// STS and SSO clients corral's AWS resolver uses, send through @smithy/node-http-handler, not fetch,
// so the guard could not see them. Adoption patches both Node handlers' prototypes to delegate to
// one FetchHttpHandler, which calls the guarded globalThis.fetch. The socket tripwire is defense in
// depth: any TCP connect or client request through Node's net, tls, http, https or http2 modules
// is refused, while Unix-socket connects (the bridge, local tools) still work.
import http from "node:http";
import http2 from "node:http2";
import https from "node:https";
import net from "node:net";
import tls from "node:tls";

import { bedrockProviderModule } from "@earendil-works/pi-ai/bedrock-provider";
import { setBedrockProviderModule } from "@earendil-works/pi-ai/compat";
import { FetchHttpHandler } from "@smithy/fetch-http-handler";
import { NodeHttp2Handler, NodeHttpHandler } from "@smithy/node-http-handler";

type Handle = (request: { headers: Record<string, string> }, options?: unknown) => Promise<unknown>;

let adopted = false;

/**
 * Routes every NodeHttpHandler and NodeHttp2Handler request through the guarded fetch, and
 * registers pi-ai's Bedrock API statically so a compiled host carries it (its lazy loader uses a
 * variable import specifier that `bun build --compile` cannot follow).
 */
export function adoptSmithyHandlers(): void {
  if (adopted) return;
  adopted = true;
  setBedrockProviderModule(bedrockProviderModule);
  const fetchHandler = new FetchHttpHandler();
  const handle: Handle = (request, options) => {
    // An HTTP/2 request is signed over its :authority pseudo-header, which an HTTP/1.1 fetch
    // cannot carry; the session forces Bedrock onto HTTP/1.1, so this only catches a regression.
    const pseudo = Object.keys(request.headers).find((name) => name.startsWith(":"));
    if (pseudo !== undefined) {
      return Promise.reject(new Error(`transport_unsupported: a request signed with ${pseudo} cannot use the guarded fetch`));
    }
    return fetchHandler.handle(request as never, options as never);
  };
  for (const handler of [NodeHttpHandler, NodeHttp2Handler]) {
    (handler.prototype as unknown as { handle: Handle }).handle = handle;
  }
}

type Options = Record<string, unknown>;

/** net.connect and friends call Socket.connect with Node's normalized [options, callback] array. */
function normalized(args: readonly unknown[]): readonly unknown[] {
  return Array.isArray(args[0]) ? args[0] as unknown[] : args;
}

function unixPath(first: unknown): boolean {
  if (typeof first === "string") return !/^\d+$/.test(first);
  return typeof first === "object" && first !== null && typeof (first as Options).path === "string";
}

function tcpTarget(args: readonly unknown[]): string {
  const [first, second] = normalized(args);
  if (typeof first === "object" && first !== null) {
    const options = first as Options;
    return `tcp://${String(options.host ?? "localhost")}:${String(options.port ?? "")}`;
  }
  return `tcp://${typeof second === "string" ? second : "localhost"}:${String(first ?? "")}`;
}

function requestTarget(protocol: string, args: readonly unknown[]): string | undefined {
  const [first, second] = args;
  if (typeof first === "string" || first instanceof URL) {
    const url = URL.parse(String(first));
    return url ? `${url.origin}${url.pathname}` : "<unparseable>";
  }
  const options = { ...(typeof first === "object" && first !== null ? first as Options : {}),
    ...(typeof second === "object" && second !== null && typeof second !== "function" ? second as Options : {}) };
  if (typeof options.socketPath === "string") return undefined;
  const host = String(options.hostname ?? options.host ?? "localhost");
  const path = String(options.path ?? "/").split("?")[0];
  return `${protocol}//${host}${options.port === undefined ? "" : `:${String(options.port)}`}${path}`;
}

let tripwire = false;

/**
 * Refuses raw TCP from this process: `refuse` records the destination in the network ledger and
 * returns the error that is thrown before any socket opens.
 */
export function installSocketTripwire(refuse: (destination: string) => Error): void {
  if (tripwire) return;
  tripwire = true;
  const connect = net.Socket.prototype.connect;
  net.Socket.prototype.connect = function (this: net.Socket, ...args: unknown[]) {
    if (unixPath(normalized(args)[0])) return (connect as (...a: unknown[]) => net.Socket).apply(this, args);
    throw refuse(tcpTarget(args));
  } as typeof connect;
  const tlsConnect = tls.connect;
  tls.connect = ((...args: unknown[]) => {
    if (unixPath(args[0])) return (tlsConnect as (...a: unknown[]) => tls.TLSSocket)(...args);
    throw refuse(tcpTarget(args));
  }) as typeof tls.connect;
  for (const [module, protocol] of [[http, "http:"], [https, "https:"]] as const) {
    for (const name of ["request", "get"] as const) {
      const original = module[name] as (...a: unknown[]) => unknown;
      (module as unknown as Record<string, unknown>)[name] = (...args: unknown[]) => {
        const target = requestTarget(protocol, args);
        if (target !== undefined) throw refuse(target);
        return original(...args);
      };
    }
  }
  http2.connect = ((authority: unknown) => {
    const url = URL.parse(String(authority));
    throw refuse(url ? `${url.origin}/` : "<unparseable>");
  }) as typeof http2.connect;
}
