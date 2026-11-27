# Corral C4 public-fixture package for Lite E1

`build.py` exports one exact **committed** Corral revision and produces a single `tar.gz` plus a JSON pin receipt on stdout. It does not build Lite, enroll either host, start a service, run E1 or grant approval. The archive uses the exact [C1 build-manifest schema](../../../docs/contracts/lite-analyst-launcher-v1/build-manifest-v1.schema.json). Only `build-manifest.json` and the listed components are present. The `provenance/launcher-v1.json` copy is convenient transport, **not** an authority for its own pin.

## Build from a pinned revision

Use a non-root Linux x86_64 Ubuntu Noble build environment matching `build-environment.json`. Restore the exact Ubuntu package versions from trusted distro snapshots rather than installing whatever is current. Provision Rust `1.94.1-x86_64-unknown-linux-gnu` and the official Bun `1.4.2` `bun-linux-x64-baseline` executable (release tag `bun-v1.4.2`, asset `bun-linux-x64-baseline.zip`); the builder checks the exact real compiler/runtime bytes. Use this recipe from the same Corral commit identified in the handoff, not a dirty locally edited recipe. Build-time Cargo/npm downloads are allowed; no resolution, install or download at launch. Build uses private fresh HOME, Cargo home and Bun cache, pinned lockfiles, no npm lifecycle scripts, no shared target, one Rust job and no ambient Cargo wrapper/default linker. The committed `.cargo/config.toml` is still in effect.

```sh
# Producer: run this from the final committed Corral revision's checkout.
export CORRAL_REVISION=<40-lowercase-hex-commit-from-handoff>
export BUN_BASELINE=/absolute/path/to/bun-linux-x64-baseline/bun
RUST_BIN=$(dirname "$(rustup which --toolchain 1.94.1-x86_64-unknown-linux-gnu cargo)")
OUT=$(mktemp -d /tmp/corral-c4-out.XXXXXXXX)
/usr/bin/python3.12 -B scripts/qa/lite_analyst_bundle/build.py \
  --revision "$CORRAL_REVISION" --rust-bin "$RUST_BIN" --bun "$BUN_BASELINE" \
  --output "$OUT/corral-fixture.tar.gz" > "$OUT/corral-pin.json"
cat "$OUT/corral-pin.json"
```

`BUILD.txt` records normalized exact argv/environment, source revision, binary toolchain/OS provenance (with the companion pinned environment JSON), lockfile/assets and patch digests, actually embedded Pi source/module digests from Bun's output metafile, and ELF runtime origins. Bun `--compile` embeds the Node-compatible runtime and the Pi JavaScript graph; the daemon executes that ELF directly, with **no** Node/Bun lookup, shell wrapper, repository, SDK/provider config or `node_modules` at launch. The recipe checks that Bun emitted one output with no imports outside the bundle. The archive includes separate read-only child and outer ELF closures. `sysroot_abi` includes a digest of the sorted runtime path → raw file digest map. The host kernel, sandbox executable and driver (and its selected runtime) are separate E1 inputs; do not treat a matching glibc version string as proof of them.

USTAR member order, permissions, ownership and timestamps and gzip metadata/level are deterministic. Rust/C path prefixes are remapped, Bun's embedded output basename is fixed, and `SOURCE_DATE_EPOCH` comes from the commit. For a reproducibility receipt build twice with the same commit and environment into separate fresh output directories; compare archive bytes (`cmp`) and reported raw manifest/whole-archive pins. A build on an unpinned toolchain is not a substitute.

## Build the separate Corral fault variant

For reviewed fault experiments only, repeat the pinned build command above with
`--with-faults` and new output names (for example `corral-fault-fixture.tar.gz`
and `corral-fault-pin.json`). This adds Cargo feature `lite-analyst-faults`,
uses `src/lite-analyst/fault-worker.ts` as Bun's entry, and copies the committed
[fault ABI](../../../docs/contracts/lite-analyst-fault-v1/README.md) to
`provenance/corral-fault-v1.md`. That asset marks the variant without changing
C1's manifest schema or entrypoint paths. The pin receipt also names
`variant: corral-fault-v1`, `fault_contract_sha256`, and the exact SHA-256 for
each of `components.daemon`, `components.pi`, and `components.shim`. The bundle
still requires independent archive and raw-manifest pins. The producer may export
the already round-trip-verified raw manifest with:

```sh
tar -xOf "$OUT/corral-fault-fixture.tar.gz" build-manifest.json > "$OUT/corral-fault-manifest.json"
```

Check its hash against the receipt before handoff. Keep these pins, raw manifest
and archive separate from the normal build and the E1 lock.

Use `verify.py --with-faults` when staging this variant. The default verifier
rejects the fault asset; fault verification rejects its absence. The same C1
argv and FDs apply, plus `--fault-fd 9 --fault-case <barrier-id>` on the Corral
daemon **only** in the fault variant. FD 9 is a preconnected `AF_UNIX` /
`SOCK_SEQPACKET` endpoint for just the fixed ASCII barrier/action records plus LF.
It does not carry Work, broker or provider messages. See the [isolated component
canaries](../lite_analyst_host/README.md#fault-variant-canaries) for a synthetic
Lite test, not a joined fault result or authorization.

## Verify and hand off

The E1 runner independently provisions the archive SHA-256, raw `build-manifest.json` SHA-256 and exact revision from the producer's authenticated/reviewed pin receipt, not from the received archive itself. It checks the complete [joint lock](../../../docs/contracts/lite-analyst-launcher-v1/joint-lock-v1.schema.json), including the independently chosen Lite build, seed eligibility, provider and external runtime tools. This helper verifies only the Corral archive, not the host's namespace preflight or the joint lock. The helper is stdlib-only and may be copied as a separate trusted runner input; it needs no checkout at launch.

```sh
# Consumer: values from the independently authenticated/pinned handoff.
export CORRAL_REVISION=<exact-commit>
export CORRAL_ARCHIVE_SHA256=<whole-archive-sha256>
export CORRAL_MANIFEST_SHA256=<raw-manifest-sha256>
export CORRAL_ARCHIVE=/absolute/path/to/corral-fixture.tar.gz
STAGE=$(mktemp -d /tmp/corral-c4-stage.XXXXXXXX)
/usr/bin/python3 -B /trusted/c4/verify.py --archive "$CORRAL_ARCHIVE" \
  --archive-sha256 "$CORRAL_ARCHIVE_SHA256" --manifest-sha256 "$CORRAL_MANIFEST_SHA256" \
  --revision "$CORRAL_REVISION" --stage "$STAGE/corral"
```

Retain the verified tree under a read-only mount through exec. C1 says exact enrollment pins, mount paths, FD roles and namespace policy; no naked invocation of the daemon suffices. On verified driver-owned descriptors, from private `/scratch`, the exact in-sandbox Corral argv is:

```text
/fixture/corral/bin/corrald-next lite-analyst-fixture-v1 --enable-public-fixture-e2e
```

Mount the package as `/fixture/corral`, the raw manifest at `/fixture/meta/build-manifest.json`, and the independently pinned profile (`c145f94492ad6683432aeb293b96be3385c1b0f7f603dfdb6e81f29de069388c`) at `/fixture/meta/launcher-v1.json`. Give only the daemon private `/state` and nested setup `/tmp`. Stage the outer loader/library paths from `runtime/host/lib64` and `runtime/host/lib` at `/lib64` and `/lib` respectively in Corral's outer namespace; no host `ld.so.cache` or broad `/usr` mount is needed there. Install the separately pinned regular non-setid Bubblewrap at `/usr/bin/bwrap`; its outer dynamic closure is in those ABI trees for the same observed binary (digest in `BUILD.txt`). The E1 runtime lock also pins the actual **external** launcher and any libraries/interpreters/runtimes it selects outside the archive; require a matching/pinned system ABI (including launcher dependencies before entering any mount namespace). For these binaries and mounts use the C1 `rootless-bubblewrap` path, including verified Landlock TCP ABI ≥4. Corral launches the two exact `bin/` children and mounts the separate `runtime/lib` and `runtime/lib64` trees under their narrower sandboxes. Do not pass the outer ABI, provider credentials or Corral state to them. FD 3 is enrollment, 4 the authenticated work socket, 5 broker bootstrap, 6 ready, 7 lifetime, 8 provider credential. No Lite, sibling checkout, provider, board, vault or owner RPC mount belongs in Corral's namespace. E1 owns the actual joint launch command once it has both packages and all independent pins.

### Corral-only package smoke

The existing isolated real daemon/Pi/shim controller can launch from these exact verified bytes (including the raw package manifest). It needs a matching host closure for its QA-only outer Python controller. Supply the separately downloaded T-2073:3 packet as an ordinary file, not a sibling repository path; it is validated against its independent C1 digest. From the Corral source containing the QA runner (which is not mounted into the sandbox):

```sh
/usr/bin/python3 -B scripts/qa/lite_analyst_host/run.py --enable-public-fixture-e2e \
  --bundle "$CORRAL_ARCHIVE" --archive-sha256 "$CORRAL_ARCHIVE_SHA256" \
  --manifest-sha256 "$CORRAL_MANIFEST_SHA256" --revision "$CORRAL_REVISION" \
  --packet /absolute/path/to/analyst-february-close-v1.json --scenario happy --scenario pending
```

This proves only the selected component lanes, including owning fake-peer fetch/ack/retention, never first-party Lite import/offline reopen or E1. Broader or failed isolation is not a passing substitute.

### Provider handoff (separate E1 input)

C1's independently pinned `/fixture/driver/bin/scripted-provider` is not part of this archive. An E1 adapter may reuse `Conversation` / `ScriptedHTTP` and `protocol.py` from `scripts/qa/lite_analyst_host/` at this revision, but must independently package and pin that complete implementation and its chosen runtime. Do **not** launch the QA controller, fake broker, or its `/state` observation callback in E1. The responder binds only `127.0.0.1:43173` in the dedicated loopback-only provider/Corral network namespace. Run from its own private scratch with a constructed environment and no vault, no product state, and just `provider_credential=3`, `ready=4`, `lifetime=5` in addition to stdio. Read exactly a 64-lowercase-hex bearer plus LF then EOF from fd 3 (same per-launch bytes separately provided to Corral); after bind send one `{"v":1,"type":"provider_ready","url":"http://127.0.0.1:43173/v1/chat/completions"}` plus LF then close fd 4. No HTTP readiness probe. A closed lifetime pipe stops the listener; handler/run bounds and ordered reaping still belong to the runner, and an interrupted provider is unavailable, never a retry/fallback trigger.

Use the exact C1 POST/SSE subset and independently pinned public prompt, policy and T-2073 packet (raw digest `12bf636c36606452e362b5c454de12284a77738fb20dfc0e7fcbfe1aab1f46a5`). Agree with Lite on the exact approved visible question before work. The reference script syntax is the packet question plus `\nApproved metric selector: <opaque-id>`; E1 must compare the question against the approved first-party value and derive the opaque selector from the **actual** visible POST question. Do not pass a hidden selector/tool result, seed report bytes, relay peer messages, or hardcode operands or handles. The script checks exact outbound prompt, ordered tool registry, prior assistant call IDs/order/args and canonical structured tool replies; turn 1 searches, turn 2 reads the live source and computes/requests the approved captured metric, turn 3 derives the report from the actual projection, terminal cell and released citations. A pending poll happens only inside Corral and is substituted as the original calculation result; no extra model turn. C1's HTTP and SSE bounds, exact tariff and observed actual send counts govern. The reference's HTTP header shape matches the current Corral transport; the C1 contract remains normative. Preserve only sanitized gate/custody/send evidence, never raw bodies or credentials.

## Publication and bounded transport

Bun alone contributes about 78 MiB uncompressed (~38 MiB gzip-1 on the build host); plan for a roughly 40–50 MiB whole compressed archive, and report the measured byte size from `corral-pin.json`. Prefer an immutable content-addressed release/file attachment with an independent authenticated pin receipt. Do not commit the archive to Git. For Corral artifact transport (20,000,000-byte default per attachment) split into 8 MiB ordered pieces, then attach the pieces, index and pin receipt. In a private producer output directory:

```sh
cd "$OUT"
LC_ALL=C split -b 8M -d -a 3 corral-fixture.tar.gz corral-fixture.part-
LC_ALL=C sha256sum corral-fixture.part-[0-9][0-9][0-9] > corral-parts.SHA256SUMS
```

In a fresh private consumer directory with just the retrieved parts and the independently pinned index (or an authenticated copy), verify ordered part names and bytes before reassembly; final authority remains `CORRAL_ARCHIVE_SHA256` and the separate manifest pin:

```sh
LC_ALL=C sha256sum --strict -c corral-parts.SHA256SUMS
# Use the authenticated index, not directory enumeration, to order the exact parts.
cut -c67- corral-parts.SHA256SUMS | while IFS= read -r part; do cat -- "$part" || exit; done > corral-fixture.tar.gz
printf '%s  %s\n' "$CORRAL_ARCHIVE_SHA256" corral-fixture.tar.gz | sha256sum --strict -c -
```

Do not accept additional assembled names, hash substitutions, or a manifest pin derived from the received archive. If immutable transport, pack verification, isolation, or final runtime evidence is unavailable, report it as blocked rather than claiming an unverified E1 pass.
