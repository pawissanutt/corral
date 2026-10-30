import { expect, test } from "bun:test";

import { checkPublicTarget, WebError } from "../src/extensions/web/policy.ts";

const publicLookup = async () => [{ address: "93.184.216.34", family: 4 }];

test("an unlisted public host is allowed by the default wildcard policy", async () => {
  // Given an arbitrary documentation host resolving publicly.
  const url = new URL("https://docs.example.org/guide");
  // When no named-host restriction narrows public access.
  await checkPublicTarget(url, ["*"], publicLookup);
  // Then its original destination is retained, without connection pinning.
  expect(url.href).toBe("https://docs.example.org/guide");
});

test("private literal and metadata targets are refused before DNS or transport", async () => {
  // Given normalized IPv4, IPv6, mapped addresses, and metadata names.
  for (const host of [
    "localhost", "localhost.", "service.localhost", "metadata.google.internal",
    "instance-data.ec2.internal", "127.0.0.1", "127.1", "2130706433", "0x7f000001",
    "10.0.0.1", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.100.100.200",
    "0.0.0.0", "224.0.0.1", "[::]", "[::1]", "[fc00::1]", "[fe80::1]",
    "[fd00:ec2::254]", "[::ffff:127.0.0.1]", "[::ffff:a00:1]",
  ]) {
    let resolved = false;
    // When the target is checked.
    const checked = checkPublicTarget(new URL(`http://${host}/`), ["*"], async () => {
      resolved = true;
      return publicLookup();
    });
    // Then even a permissive resolver cannot authorize it.
    await expect(checked).rejects.toThrow(WebError);
    expect(resolved).toBeFalse();
  }
});

test("DNS refuses any private answer even alongside public addresses", async () => {
  // Given mixed answers, including IPv6 and mapped private addresses.
  for (const address of ["10.1.2.3", "::1", "::ffff:192.168.1.1", "fe80::1"]) {
    // When an ordinary hostname resolves to both public and private addresses.
    const checked = checkPublicTarget(new URL("https://docs.example.org/"), ["*"],
      async () => [...await publicLookup(), { address, family: address.includes(":") ? 6 : 4 }]);
    // Then no connection may start.
    await expect(checked).rejects.toThrow(WebError);
  }
});

test("public IPv6 and mapped public IPv4 addresses are usable", async () => {
  // Given both globally reachable address forms.
  for (const address of ["2606:4700:4700::1111", "::ffff:93.184.216.34"]) {
    // When ordinary resolution returns them.
    await checkPublicTarget(new URL("https://docs.example.org/"), ["*"], async () => [{ address, family: 6 }]);
    // Then resolution succeeded without changing the hostname.
  }
});

test("named hosts narrow fetch without authorizing private targets or subdomains", async () => {
  // Given a named-host list, with URL normalization for case and a terminal dot.
  await checkPublicTarget(new URL("https://DOCS.EXAMPLE.ORG./"), ["docs.example.org"], publicLookup);
  // When another host or a subdomain is requested.
  for (const host of ["other.example.org", "child.docs.example.org"]) {
    // Then it is refused even though it resolves publicly.
    await expect(checkPublicTarget(new URL(`https://${host}/`), ["docs.example.org"], publicLookup))
      .rejects.toThrow(WebError);
  }
  await expect(checkPublicTarget(new URL("http://127.0.0.1/"), ["127.0.0.1"], publicLookup))
    .rejects.toThrow(WebError);
});

test("non-HTTP targets and embedded credentials are refused", async () => {
  // Given destinations outside the GET-only no-credentials contract.
  for (const target of ["file:///etc/passwd", "ftp://example.org/", "https://user:secret@example.org/"]) {
    // When checked, then they never become network destinations.
    await expect(checkPublicTarget(new URL(target), ["*"], publicLookup)).rejects.toThrow(WebError);
  }
});
