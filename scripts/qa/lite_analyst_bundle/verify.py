#!/usr/bin/env python3
"""Verify/stage the Corral C4 package, not a joint runner or isolation grant."""
import argparse
import gzip
import hashlib
import json
import os
import re
import shutil
import tarfile
import tempfile
from pathlib import Path

PROFILE_SHA = "c145f94492ad6683432aeb293b96be3385c1b0f7f603dfdb6e81f29de069388c"
PROMPT_SHA = "103ccacd5a80abbd9f83e4d64441cbb6b07e5d7a8c28c793d15bc16aaded8f02"
RAW_POLICY_SHA = "38fae11185d2b48302d026ed788b04f1e5e6340fced8aa08f9d24707f9607de5"
JCS_POLICY_SHA = "8879af6aeb66bf98b2533bbe6c99b12c5c5949e37d71622c0267277843f96b4d"
ENTRYPOINTS = {"daemon": "bin/corrald-next", "pi": "bin/corral-pi-lite-analyst", "shim": "bin/corral-lite-mcp-shim"}
ROLES = {*ENTRYPOINTS, "runtime", "asset", "build_recipe", "prompt", "tool_policy"}
ASSETS = {"prompt": ("assets/synthetic-prompt.txt", PROMPT_SHA),
          "tool_policy": ("assets/tool-policy.json", RAW_POLICY_SHA)}
MANIFEST = "build-manifest.json"
PATH = re.compile(r"[A-Za-z0-9_-][A-Za-z0-9_.-]*(/[A-Za-z0-9_-][A-Za-z0-9_.-]*)*")
ABI_PREFIX = "glibc-2.39-noble-elf-closure-sha256:"
ARCHIVE_LIMIT = 256 << 20
MANIFEST_LIMIT = 16 << 20
TOTAL_LIMIT = 512 << 20


def need(condition, error):
    if not condition:
        raise ValueError(error)


def canonical(obj):
    # JCS for the pinned policy's ASCII keys and safe integers, not a general JCS encoder.
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()


def strict_json(raw):
    def pairs(fields):
        result = {}
        for key, value in fields:
            need(key not in result, "duplicate JSON key")
            result[key] = value
        return result

    def invalid(_):
        raise ValueError("non-integer JSON")

    return json.loads(raw.decode("utf-8"), object_pairs_hook=pairs, parse_float=invalid, parse_constant=invalid)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def file_sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def match(value, regex):
    return type(value) is str and re.fullmatch(regex, value) is not None


def sha_ok(value):
    return match(value, r"[0-9a-f]{64}")


def path_ok(value):
    return match(value, PATH) and len(value) <= 256


def keys(obj, *names):
    return type(obj) is dict and obj.keys() == set(names)


def runtime_abi(components):
    return ABI_PREFIX + sha(canonical({p: c["sha256"] for p, c in components.items() if c["role"] == "runtime"}))


def validate(raw, revision):
    m = strict_json(raw)
    need(keys(m, "v", "kind", "status", "target", "corral_revision", "launcher_profile_sha256", "build_recipe",
              "host_requirements", "entrypoints", "components")
         and type(m["v"]) is int and m["v"] == 1 and m["kind"] == "corral_lite_fixture_build"
         and m["status"] == "runnable" and m["target"] == "linux-x86_64"
         and match(revision, r"[0-9a-f]{40}") and int(revision, 16) != 0 and m["corral_revision"] == revision
         and m["launcher_profile_sha256"] == PROFILE_SHA and m["entrypoints"] == ENTRYPOINTS
         and m["build_recipe"] == "BUILD.txt", "wrong build identity, schema or profile")
    components = m["components"]
    need(type(components) is dict and 7 <= len(components) <= 16384 and MANIFEST not in components,
         "invalid component map")
    for p, c in components.items():
        need(path_ok(p) and keys(c, "role", "sha256", "bytes", "executable") and type(c["role"]) is str
             and c["role"] in ROLES and sha_ok(c["sha256"]) and type(c["bytes"]) is int
             and 0 < c["bytes"] <= 1073741824 and type(c["executable"]) is bool, "invalid component")
    for role, path in ENTRYPOINTS.items():
        need([p for p, c in components.items() if c["role"] == role] == [path] and components[path]["executable"],
             "invalid entrypoint role or mode")
    for role, (path, digest) in ASSETS.items():
        need([p for p, c in components.items() if c["role"] == role] == [path]
             and components[path]["sha256"] == digest and not components[path]["executable"], "invalid asset pin")
    need([p for p, c in components.items() if c["role"] == "build_recipe"] == [m["build_recipe"]]
         and not components[m["build_recipe"]]["executable"], "invalid build recipe")
    req = m["host_requirements"]
    need(keys(req, "landlock_tcp_abi_min", "sysroot_abi", "sandbox_backend")
         and type(req["landlock_tcp_abi_min"]) is int and req["landlock_tcp_abi_min"] == 4
         and req["sandbox_backend"] == "rootless-bubblewrap" and req["sysroot_abi"] == runtime_abi(components),
         "invalid ABI or backend")
    need(all(components.get(p, {}).get("role") == "runtime" for p in (
        "runtime/lib64/ld-linux-x86-64.so.2", "runtime/host/lib64/ld-linux-x86-64.so.2")), "missing bundled loader")
    profile = components.get("provenance/launcher-v1.json", {})
    need(profile.get("role") == "asset" and profile.get("sha256") == PROFILE_SHA and not profile["executable"],
         "missing pinned launcher profile")
    need(sum(c["bytes"] for c in components.values()) <= TOTAL_LIMIT, "package exceeds local size limit")
    return m


def staged_copy(archive, destination, archive_pin, manifest_pin, revision):
    """Read the archive only through a private hash-checked snapshot; caller mounts read-only."""
    need(sha_ok(archive_pin) and sha_ok(manifest_pin), "archive and raw manifest pins required")
    dest = Path(destination)
    # Caller keeps this private root until read-only mounts and teardown are confirmed.
    parent = dest.parent.lstat()
    need(parent.st_uid == os.getuid() and parent.st_mode == 0o40700, "private staging parent required")
    dest.mkdir(mode=0o700)
    try:
        with tempfile.TemporaryFile(dir=dest) as checked, Path(archive).open("rb") as source:
            total, digest = 0, hashlib.sha256()
            while data := source.read(1 << 20):
                total += len(data)
                need(total <= ARCHIVE_LIMIT, "archive exceeds local size limit")
                digest.update(data)
                checked.write(data)
            need(digest.hexdigest() == archive_pin, "archive digest mismatch")
            checked.seek(0)
            with gzip.GzipFile(fileobj=checked) as tar:
                extract(tar, dest, manifest_pin, revision)
        # Check both exact raw custody (above) and the policy's canonical pin.
        policy = dest / ASSETS["tool_policy"][0]
        need(sha(canonical(strict_json(policy.read_bytes()))) == JCS_POLICY_SHA, "canonical policy mismatch")
    except BaseException:
        shutil.rmtree(dest)
        raise


def exact(reader, size):
    data = reader.read(size)
    need(len(data) == size, "truncated archive")
    return data


def padding(reader, size):
    need(not any(exact(reader, -size % 512)), "nonzero archive padding")


def extract(tar, dest, manifest_pin, revision):
    # Parse physical USTAR headers, so library normalization cannot hide long-name,
    # link, sparse or PAX extension records, aliases or trailing extra members.
    def header():
        raw = exact(tar, 512)
        need(raw[156:157] == tarfile.REGTYPE, "non-regular archive member")
        entry = tarfile.TarInfo.frombuf(raw, "ascii", "strict")
        need(not entry.linkname and entry.tobuf(format=tarfile.USTAR_FORMAT, encoding="ascii") == raw,
             "not canonical USTAR")
        return entry

    info = header()
    need(info.name == MANIFEST and info.mode == 0o444 and 0 < info.size <= MANIFEST_LIMIT,
         "missing initial plain build manifest")
    raw = exact(tar, info.size)
    padding(tar, info.size)
    need(sha(raw) == manifest_pin, "raw manifest digest mismatch")
    components = validate(raw, revision)["components"]
    (dest / MANIFEST).write_bytes(raw)
    (dest / MANIFEST).chmod(0o444)
    found = set()
    for _ in components:
        info = header()
        p = info.name
        need(p in components and p not in found, "extra or duplicate archive member")
        found.add(p)
        entry = components[p]
        mode = 0o555 if entry["executable"] else 0o444
        need(info.size == entry["bytes"] and info.mode == mode, "component size or mode mismatch")
        out = dest / p
        out.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        digest, remaining = hashlib.sha256(), info.size
        with out.open("xb") as f:
            while remaining:
                data = exact(tar, min(remaining, 1 << 20))
                digest.update(data)
                f.write(data)
                remaining -= len(data)
        need(digest.hexdigest() == entry["sha256"], "component digest mismatch")
        padding(tar, info.size)
        out.chmod(mode)
    remaining = tar.read(1024 + tarfile.RECORDSIZE + 1)
    need(1024 <= len(remaining) < 1024 + tarfile.RECORDSIZE and not any(remaining), "extra members or invalid archive end")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--archive", type=Path, required=True)
    p.add_argument("--archive-sha256", required=True)
    p.add_argument("--manifest-sha256", required=True)
    p.add_argument("--revision", required=True)
    p.add_argument("--stage", type=Path, required=True, help="new directory inside a private staging root")
    a = p.parse_args()
    try:
        staged_copy(a.archive, a.stage, a.archive_sha256, a.manifest_sha256, a.revision)
    except (EOFError, OSError, ValueError, RecursionError, tarfile.TarError) as e:
        p.exit(64, f"Corral package rejected: {e}\n")


if __name__ == "__main__":
    main()
