#!/usr/bin/env python3
"""Build the public-only Linux Corral fixture from one committed source revision."""
import argparse
import gzip
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

from verify import (ARCHIVE_LIMIT, ASSETS, ENTRYPOINTS, FAULT_ASSET, FAULT_VARIANT, MANIFEST, PROFILE_SHA,
                    canonical, file_sha, need, runtime_abi, sha, staged_copy, strict_json, validate)

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent / "lite_analyst_host"))
from staging import BWRAP, LOADER, elf_dependencies, linux_mount_path

TARGET = "x86_64-unknown-linux-gnu"
PI_TARGET = "bun-linux-x64-baseline"
PI_VERSION = "1.4.2"
PI_RUNTIME = f"{PI_TARGET}-v{PI_VERSION}"
PI_FLAGS = ("--compile", f"--target={PI_TARGET}", "--no-compile-autoload-dotenv", "--no-compile-autoload-bunfig",
            "--no-compile-autoload-tsconfig", "--no-compile-autoload-package-json")
ENVIRONMENT = "scripts/qa/lite_analyst_bundle/build-environment.json"
FAULT_ENVIRONMENT = "scripts/qa/lite_analyst_bundle/build-environment-fault-v1.json"
SOURCE_PINS = {"docs/contracts/lite-analyst-launcher-v1/launcher-v1.json": PROFILE_SHA,
               "docs/contracts/lite-analyst-launcher-v1/build-manifest-v1.schema.json":
                   "77ded5039e6fd3581762f119b22115cd364981d3a58054cc9a01226f543a0d3a"}


def run(argv, *, cwd, env, capture=False):
    return subprocess.run(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL, check=True,
                          stdout=subprocess.PIPE if capture else sys.stderr).stdout


def source_archive(revision, root, env):
    git = ("/usr/bin/git", "--no-replace-objects", "-c", "core.attributesFile=/dev/null")
    opts = {"cwd": REPO, "env": env, "check": True, "stdin": subprocess.DEVNULL}
    actual, epoch = subprocess.check_output([*git, "show", "--no-patch", "--format=%H %ct", revision],
                                           stdin=subprocess.DEVNULL, cwd=REPO, env=env).decode("ascii").split()
    need(actual == revision and epoch.isdecimal(), "exact Git commit required")
    export = root / "source.tar"
    with export.open("xb") as f:
        subprocess.run([*git, "archive", "--format=tar", revision], stdout=f, **opts)
    with tarfile.open(export) as tar:
        tar.extractall(root / "source", filter="data")
    return root / "source", epoch


def check_environment(source, tools, env, environment):
    expected = strict_json((source / environment).read_bytes())
    for name, binary in {**tools, "loader": Path(LOADER)}.items():
        need(file_sha(binary) == expected[f"{name}_sha256"], f"{name} SHA-256 differs from build pin")
    names = sorted(expected["packages"])
    observed = run(["/usr/bin/dpkg-query", "-W", "-f=${Package}=${Version}\\n", *names],
                   cwd=source, env=env, capture=True).decode("ascii").splitlines()
    need(sorted(observed) == sorted(f"{k}={v}" for k, v in expected["packages"].items()),
         f"build a provisioned Ubuntu 24.04 sysroot matching {Path(environment).name}")


def compile_programs(root, source, tools, env, *, with_faults=False):
    pi = source / "packages/corral-pi"
    cargo = str(tools["cargo"])
    bun = str(tools["bun"])
    (root / "bun-cache").mkdir()
    # Same pinned baseline runtime as the compiler; prevents a compile-time runtime download.
    shutil.copyfile(bun, root / "bun-cache" / PI_RUNTIME)
    (root / "bun-cache" / PI_RUNTIME).chmod(0o555)
    features = ["--features", "lite-analyst-faults"] if with_faults else []
    pi_entry = "src/lite-analyst/fault-worker.ts" if with_faults else "src/lite-analyst/main.ts"
    commands = [
        (source, [cargo, "fetch", "--locked", "--target", TARGET]),
        (pi, [bun, "install", "--frozen-lockfile", "--ignore-scripts"]),
        (source, [cargo, "build", "--frozen", "--release", "--target", TARGET, "-j1", "-p", "corral-daemon",
                  "--bin", "corrald-next", "--bin", "corral-lite-mcp-shim", *features]),
        (pi, [bun, "build", pi_entry, *PI_FLAGS, f"--metafile={root}/pi-metafile.json",
              f"--outfile={root}/bin/corral-pi-lite-analyst"]),
    ]
    for directory, argv in commands:
        run(argv, cwd=directory, env=env)
    meta = strict_json((root / "pi-metafile.json").read_bytes())
    outputs = list(meta["outputs"].values())
    need(len(outputs) == 1 and not outputs[0]["imports"], "Pi build emitted outside runtime dependencies")
    # Audit the actual emitted sources (not the unused dev/provider dependency graph).
    inputs = outputs[0]["inputs"]
    need(pi_entry in inputs and ("src/lite-analyst/fault-worker.ts" in inputs) == with_faults
         and all(p.startswith(("src/lite-analyst/", "node_modules/@earendil-works/")) for p in inputs),
         "review changed Pi bundle closure")
    compile_inputs = {p: file_sha(pi / p) for p in inputs}
    rust_output = root / "target" / TARGET / "release"
    programs = {"pi": root / "bin/corral-pi-lite-analyst", "daemon": rust_output / "corrald-next",
                "shim": rust_output / "corral-lite-mcp-shim"}
    aliases = {str(root): "$BUILD_ROOT", cargo: "$RUST_BIN/cargo", str(tools["rustc"]): "$RUST_BIN/rustc", bun: "$BUN"}
    def template(arg):
        for path, alias in aliases.items():
            arg = arg.replace(path, alias)
        return arg
    recipe_commands = [{"cwd": template(str(cwd)), "argv": [template(a) for a in argv]} for cwd, argv in commands]
    selected_env = {key: template(value) for key, value in env.items() if key not in ("GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL")}
    return programs, compile_inputs, recipe_commands, selected_env


def archive(pkg, output, manifest):
    # Names, order, tar/gzip metadata and compression level are fixed, not inherited from the filesystem.
    with output.open("xb") as f, gzip.GzipFile(filename="", fileobj=f, mode="wb", mtime=0, compresslevel=6) as gz:
        with tarfile.open(fileobj=gz, mode="w|", format=tarfile.USTAR_FORMAT) as tar:
            for p in (MANIFEST, *sorted(manifest["components"])):
                path = pkg / p
                info = tarfile.TarInfo(p)
                info.mode = stat.S_IMODE(path.stat().st_mode)
                info.size = path.stat().st_size
                with path.open("rb") as data:
                    tar.addfile(info, data)


def package(root, source, revision, programs, provenance, *, with_faults=False):
    pkg = root / "package"
    components = {}
    runtime_origins = {}

    def add(p, role, *, source_file=None, data=None, executable=False):
        dest = pkg / p
        dest.parent.mkdir(parents=True, exist_ok=True)
        if source_file is not None:
            shutil.copyfile(source_file, dest)
        else:
            dest.write_bytes(data)
        dest.chmod(0o555 if executable else 0o444)
        components[p] = {"role": role, "bytes": dest.stat().st_size, "sha256": file_sha(dest),
                         "executable": executable}

    def runtime(src, base):
        for dependency in elf_dependencies(src):
            dest = base + "/" + linux_mount_path(dependency).as_posix()
            need(dest.startswith((base + "/lib/", base + "/lib64/")), "outside ELF library mounts")
            if dest in components:
                need(components[dest]["sha256"] == file_sha(dependency), "different bytes at same ELF library path")
            else:
                runtime_origins[dest] = str(dependency)
                add(dest, "runtime", source_file=dependency, executable=bool(dependency.stat().st_mode & 0o111))

    for role, p in ENTRYPOINTS.items():
        add(p, role, source_file=programs[role], executable=True)
        runtime(pkg / p, "runtime/host" if role == "daemon" else "runtime")
    # The host sandbox also execs this launcher from the exact /usr/bin/bwrap path.
    runtime(BWRAP, "runtime/host")
    for role, (p, digest) in ASSETS.items():
        src = source / "docs/contracts/lite-analyst-v1" / Path(p).name
        need(file_sha(src) == digest, "upstream asset bytes changed")
        add(p, role, source_file=src)
    for p, digest in SOURCE_PINS.items():
        need(file_sha(source / p) == digest, "upstream launcher input bytes changed")
        add(f"provenance/{Path(p).name}", "asset", source_file=source / p)
    if with_faults:
        add(FAULT_ASSET, "asset", source_file=source / "docs/contracts/lite-analyst-fault-v1/README.md")
        provenance["variant"] = FAULT_VARIANT
    for p in (FAULT_ENVIRONMENT if with_faults else ENVIRONMENT,
              "Cargo.lock", ".cargo/config.toml", "packages/corral-pi/bun.lock",
              "packages/corral-pi/package.json", "LICENSE"):
        add(f"provenance/{Path(p).name}", "asset", source_file=source / p)
    provenance.update({"corral_revision": revision, "launcher_profile_sha256": PROFILE_SHA,
                       "runtime_source_paths": runtime_origins, "sandbox_observed_sha256": file_sha(BWRAP),
                       "input_sha256": {p: c["sha256"] for p, c in components.items() if c["role"] in (
                           "asset", "prompt", "tool_policy")},
                       "bun_patch_sha256": {p.name: file_sha(p) for p in sorted((source / "packages/corral-pi/patches").glob("*.patch"))}})
    add("BUILD.txt", "build_recipe", data=canonical(provenance) + b"\n")
    manifest = {"v": 1, "kind": "corral_lite_fixture_build", "status": "runnable", "target": "linux-x86_64",
                "corral_revision": revision, "launcher_profile_sha256": PROFILE_SHA, "build_recipe": "BUILD.txt",
                "entrypoints": ENTRYPOINTS, "components": components,
                "host_requirements": {"landlock_tcp_abi_min": 4, "sysroot_abi": runtime_abi(components),
                                      "sandbox_backend": "rootless-bubblewrap"}}
    raw = canonical(manifest) + b"\n"
    validate(raw, revision, with_faults=with_faults)
    (pkg / MANIFEST).write_bytes(raw)
    (pkg / MANIFEST).chmod(0o444)
    return pkg, manifest, sha(raw)


def build(a):
    need(platform.system() == "Linux" and platform.machine() == "x86_64" and os.geteuid() != 0,
         "non-root Linux x86_64 build required")
    need(re.fullmatch(r"[0-9a-f]{40}", a.revision) and int(a.revision, 16), "full nonzero Corral revision required")
    need(not a.output.exists(), "output already exists")
    with tempfile.TemporaryDirectory(prefix="corral-c4-build-", dir="/tmp") as directory:
        root = Path(directory)
        for name in ("home", "tmp", "cargo-home", "target", "bin"):
            (root / name).mkdir()
        tools = {"cargo": (a.rust_bin / "cargo").resolve(strict=True),
                 "rustc": (a.rust_bin / "rustc").resolve(strict=True), "bun": a.bun.resolve(strict=True)}
        env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC", "HOME": str(root / "home"),
               "TMPDIR": str(root / "tmp"), "CARGO_HOME": str(root / "cargo-home"),
               "CARGO_TARGET_DIR": str(root / "target"), "BUN_INSTALL_CACHE_DIR": str(root / "bun-cache"),
               "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "RUSTC": str(tools["rustc"]),
               "CC": "/usr/bin/cc", "AR": "/usr/bin/ar", "LIBCURL_NO_PKG_CONFIG": "1",
               "PKG_CONFIG_LIBDIR": "/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig",
               "CARGO_PROFILE_RELEASE_DEBUG": "0", "CARGO_PROFILE_RELEASE_STRIP": "symbols", "CARGO_INCREMENTAL": "0",
               "RUSTFLAGS": f"--remap-path-prefix={root}=/build -C linker=/usr/bin/cc -C linker-features=-lld",
               "CFLAGS": f"-ffile-prefix-map={root}=/build"}
        source, epoch = source_archive(a.revision, root, env)
        env["SOURCE_DATE_EPOCH"] = epoch
        environment = FAULT_ENVIRONMENT if a.with_faults else ENVIRONMENT
        names = ("build.py", "verify.py", Path(environment).name, "README.md")
        for p in ([HERE / n for n in names] + [HERE.parent / "lite_analyst_host" / n for n in ("staging.py", "protocol.py")]):
            need(file_sha(source / p.relative_to(REPO)) == file_sha(p), "run the builder from the pinned commit")
        check_environment(source, tools, env, environment)
        programs, pi_inputs, commands, compile_env = compile_programs(root, source, tools, env, with_faults=a.with_faults)
        provenance = {"purpose": "C4 public-only Corral build; not a joint E1 result or authorization",
                      "recipe": "scripts/qa/lite_analyst_bundle/README.md at corral_revision",
                      "commands": commands, "build_environment": compile_env,
                      "pi_runtime": PI_RUNTIME, "pi_embedded_input_sha256": pi_inputs,
                      "archive_format": "manifest-first USTAR, gzip level 6 mtime 0; no directory records"}
        pkg, manifest, manifest_pin = package(root, source, a.revision, programs, provenance, with_faults=a.with_faults)
        built = root / "corral-fixture.tar.gz"
        archive(pkg, built, manifest)
        need(built.stat().st_size <= ARCHIVE_LIMIT, "archive exceeds local size limit")
        archive_pin = file_sha(built)
        staged_copy(built, root / "roundtrip", archive_pin, manifest_pin, a.revision, with_faults=a.with_faults)
        with built.open("rb") as src, a.output.open("xb") as dest:
            shutil.copyfileobj(src, dest)
        receipt = {"revision": a.revision, "archive_sha256": archive_pin, "manifest_sha256": manifest_pin,
                   "archive_bytes": built.stat().st_size, "launcher_profile_sha256": PROFILE_SHA}
        if a.with_faults:
            components = manifest["components"]
            receipt.update(variant=FAULT_VARIANT, fault_contract_sha256=components[FAULT_ASSET]["sha256"],
                           components={role: components[p]["sha256"] for role, p in ENTRYPOINTS.items()})
        return receipt


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--revision", required=True, help="exact committed Corral revision with this recipe")
    p.add_argument("--rust-bin", required=True, type=Path, help="pinned Rust toolchain's real bin directory")
    p.add_argument("--bun", required=True, type=Path, help="pinned standalone Bun baseline executable")
    p.add_argument("--output", required=True, type=Path, help="new .tar.gz file in a private output directory")
    p.add_argument("--with-faults", action="store_true", help="build the separate Corral fault variant, not E1 bytes")
    a = p.parse_args()
    try:
        print(canonical(build(a)).decode())
    except (OSError, ValueError, subprocess.SubprocessError, tarfile.TarError) as e:
        p.exit(1, f"build failed: {e}\n")


if __name__ == "__main__":
    main()
