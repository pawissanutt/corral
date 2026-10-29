import { lookup } from "node:dns/promises";
import { BlockList, isIP } from "node:net";

export class WebError extends Error {
  constructor(readonly code: string, message: string) {
    super(`${code}: ${message}`);
  }
}

export type Lookup = (hostname: string) => Promise<readonly { readonly address: string; readonly family: number }[]>;
export const resolveHost: Lookup = (hostname) => lookup(hostname, { all: true, verbatim: true });

const privateV4 = new BlockList();
for (const [address, prefix] of [
  ["0.0.0.0", 8], ["10.0.0.0", 8], ["100.64.0.0", 10], ["127.0.0.0", 8],
  ["169.254.0.0", 16], ["172.16.0.0", 12], ["192.0.0.0", 24], ["192.0.2.0", 24],
  ["192.168.0.0", 16], ["198.18.0.0", 15], ["198.51.100.0", 24], ["203.0.113.0", 24],
  ["224.0.0.0", 4], ["240.0.0.0", 4],
] as const) privateV4.addSubnet(address, prefix, "ipv4");
const globalV6 = new BlockList();
globalV6.addSubnet("2000::", 3, "ipv6");
const reservedV6 = new BlockList();
reservedV6.addSubnet("2001:db8::", 32, "ipv6");

function publicAddress(address: string): boolean {
  const family = isIP(address);
  if (family === 4) return !privateV4.check(address, "ipv4");
  if (family !== 6) return false;
  const mapped = /^::ffff:(.+)$/i.exec(address)?.[1];
  if (mapped) {
    if (isIP(mapped) === 4) return publicAddress(mapped);
    const words = /^([0-9a-f]{1,4}):([0-9a-f]{1,4})$/i.exec(mapped);
    if (words?.[1] && words[2]) {
      const high = Number.parseInt(words[1], 16);
      const low = Number.parseInt(words[2], 16);
      return publicAddress(`${high >>> 8}.${high & 255}.${low >>> 8}.${low & 255}`);
    }
    return false;
  }
  return globalV6.check(address, "ipv6") && !reservedV6.check(address, "ipv6");
}

function hostname(host: string): string {
  return host.replace(/^\[|\]$/g, "").replace(/\.$/, "").toLowerCase();
}

/** Ordinary URL/IP/DNS checks only: resolution is not pinned to the subsequent connection. */
export async function checkPublicTarget(url: URL, hosts: readonly string[], resolve: Lookup = resolveHost): Promise<void> {
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) {
    throw new WebError("web_target_refused", "use an HTTP(S) URL without credentials");
  }
  const host = hostname(url.hostname);
  if (!hosts.includes("*") && !hosts.some((name) => hostname(name) === host)) {
    throw new WebError("web_target_refused", `${host} is outside web.hosts`);
  }
  if (host === "localhost" || host.endsWith(".localhost") || host === "localhost.localdomain"
      || host === "metadata.google.internal" || host === "instance-data.ec2.internal") {
    throw new WebError("web_target_refused", `${host} is a local or metadata target`);
  }
  if (isIP(host)) {
    if (!publicAddress(host)) throw new WebError("web_target_refused", `${host} is not a public address`);
    return;
  }
  const answers = await resolve(host);
  if (!answers.length || answers.some(({ address }) => !publicAddress(address))) {
    throw new WebError("web_target_refused", `${host} does not resolve exclusively to public addresses`);
  }
}
