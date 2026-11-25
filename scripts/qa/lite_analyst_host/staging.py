"""Stage and pin only the selected source assets and executable/runtime bytes."""
import re
import shutil
import stat
import subprocess
import sys
import tarfile
from pathlib import Path

from protocol import PACKET_SHA, PROFILE_SHA, PROMPT_SHA, RAW_POLICY_SHA, SCENARIOS, checked_file, file_sha, load, need, sha, wire

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
BWRAP = Path("/usr/bin/bwrap")
LOADER = "/lib64/ld-linux-x86-64.so.2"
CLEAN_ENV = {"PATH": "/usr/bin", "LC_ALL": "C"}
ENTRYPOINTS = {"daemon": "bin/corrald-next", "pi": "bin/corral-pi-lite-analyst", "shim": "bin/corral-lite-mcp-shim"}
QA_FILES = ("inside.py", "approval.py", "child.py", "environment.py", "peer.py", "responder.py", "protocol.py")


def checked_program(path):
    mode = path.stat().st_mode
    need(stat.S_ISREG(mode) and mode & 0o111 and not mode & (stat.S_ISUID | stat.S_ISGID), "input_not_plain_executable")


def install(src, dest, executable=False):
    dest.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    shutil.copyfile(src, dest)
    dest.chmod(0o555 if executable else 0o444)


def tool_output(argv):
    result = subprocess.run(argv, env=CLEAN_ENV, cwd="/tmp", stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=10, check=False)
    need(result.returncode == 0 and len(result.stdout) <= 65536, "elf_dependency_tool")
    return result.stdout.decode("ascii")


def elf_dependencies(path):
    header = tool_output(["/usr/bin/readelf", "-h", "-l", path])
    need("Advanced Micro Devices X86-64" in header and "ELF64" in header, "not_x86_64_elf")
    match = re.search(r"\[Requesting program interpreter: ([^\]]+)\]", header)
    if match is None:
        return []  # Statically linked ELF.
    need(match[1] == LOADER, "unsupported_elf_interpreter")
    # Invoke the trusted system loader in --list mode, never the staged program.
    # No LD_*, host ld.so cache, shell evaluation or caller-controlled RPATH root.
    deps = [Path(LOADER)]
    for line in tool_output([LOADER, "--inhibit-cache", "--list", path]).splitlines():
        fields = line.split()
        if not fields or fields[0].startswith("linux-vdso.so"):
            continue
        selected = fields[2] if len(fields) >= 4 and fields[1] == "=>" else fields[0]
        dep = Path(selected)
        need(dep.is_absolute() and dep.parts[:2] in (("/", "lib"), ("/", "lib64"), ("/", "usr"))
             and dep.resolve().is_relative_to("/usr/lib") and dep.is_file(), "elf_dependency_outside_sysroot")
        if len(fields) >= 4 and fields[1] == "=>":
            need(dep.name == fields[0], "elf_dependency_soname")
        deps.append(dep)
    return sorted(set(deps))


def linux_mount_path(dep):
    if dep.parts[1] == "usr":
        return Path(*dep.parts[2:])
    return Path(*dep.parts[1:])


def stage_local(pkg, opts, loader_pin):
    bundle = REPO / "docs/contracts/lite-analyst-v1"
    runtime = pkg / "runtime"
    components = {}

    def add(path, role, executable):
        obj = pkg / path
        components[str(path)] = {"role": role, "sha256": file_sha(obj), "bytes": obj.stat().st_size,
                                 "executable": executable}

    for role, dest in ENTRYPOINTS.items():
        src = getattr(opts, role).resolve(strict=True)
        checked_program(src)
        install(src, pkg / dest, True)
        add(dest, role, True)
        deps = elf_dependencies(pkg / dest)
        if role == "daemon":
            continue  # Outer disposable /usr is allowed, never mounted in Pi/shim.
        for dep in deps:
            dest = Path("runtime") / linux_mount_path(dep)
            need(dest.parts[1] in ("lib", "lib64"), "elf_runtime_mount")
            if str(dest) in components:
                need(components[str(dest)]["sha256"] == file_sha(dep), "elf_soname_collision")
                continue
            executable = bool(dep.stat().st_mode & 0o111)
            install(dep, pkg / dest, executable)
            add(dest, "runtime", executable)
    for directory in ("lib", "lib64"):
        (runtime / directory).mkdir(mode=0o700, parents=True, exist_ok=True)
    for name, role, digest in (("synthetic-prompt.txt", "prompt", PROMPT_SHA),
                               ("tool-policy.json", "tool_policy", RAW_POLICY_SHA)):
        checked_file(bundle / name, digest)
        dest = f"assets/{name}"
        install(bundle / name, pkg / dest)
        add(dest, role, False)
    recipe = pkg / "BUILD.txt"
    recipe.write_text("Component QA inputs; revision declared by caller, exact binaries pinned by staged hashes.\n"
                      "Expected: cargo build --locked -p corral-daemon --bins;\n"
                      "bun build --compile --no-compile-autoload-dotenv --no-compile-autoload-bunfig "
                      "--no-compile-autoload-tsconfig --no-compile-autoload-package-json ...\n"
                      "No build or dependency resolution during fixture launch. Not a C4 or E1 build.\n")
    recipe.chmod(0o444)
    add("BUILD.txt", "build_recipe", False)
    manifest = {"v": 1, "kind": "corral_lite_fixture_build", "status": "runnable", "target": "linux-x86_64",
                "corral_revision": opts.revision, "launcher_profile_sha256": PROFILE_SHA, "build_recipe": "BUILD.txt",
                "host_requirements": {"landlock_tcp_abi_min": 4, "sysroot_abi": "qa-ld-linux:" + loader_pin,
                                      "sandbox_backend": "rootless-bubblewrap"},
                "entrypoints": ENTRYPOINTS, "components": components}
    return wire(manifest), components


def stage_bundle(pkg, opts, host):
    sys.path.insert(0, str(HERE.parent / "lite_analyst_bundle"))
    from verify import staged_copy, validate

    try:
        staged_copy(opts.bundle, pkg, opts.archive_sha256, opts.manifest_sha256, opts.revision)
    except (EOFError, OSError, ValueError, tarfile.TarError):
        need(False, "c4_archive_or_manifest_pin_invalid")
    raw = (pkg / "build-manifest.json").read_bytes()
    components = validate(raw, opts.revision)["components"]
    recipe = load((pkg / "BUILD.txt").read_bytes())
    need(recipe["sandbox_observed_sha256"] == host["bwrap_sha256"], "c4_bundle_sandbox_abi_mismatch")
    # The component controller needs its host Python stdlib in /usr. Require the
    # outer sandbox's read-only /usr to supply the same verified ELF closure.
    base = "runtime/host/"
    for path, pin in components.items():
        if path.startswith(base):
            need(file_sha(Path("/" + path[len(base):])) == pin["sha256"], "c4_bundle_host_abi_mismatch")
    return raw, components


def stage(root, opts):
    qa = root / "qa"
    meta = root / "meta"
    pkg = root / "corral"
    host = {"bwrap_sha256": file_sha(BWRAP), "loader_sha256": file_sha(Path(LOADER))}
    profile = REPO / "docs/contracts/lite-analyst-launcher-v1/launcher-v1.json"
    if opts.bundle:
        raw, components = stage_bundle(pkg, opts, host)
        profile = pkg / "provenance/launcher-v1.json"
    else:
        raw, components = stage_local(pkg, opts, host["loader_sha256"])
    checked_file(profile, PROFILE_SHA)
    checked_file(opts.packet, PACKET_SHA)
    install(profile, meta / "launcher-v1.json")
    install(opts.packet, meta / "february.json")
    for file in QA_FILES:
        install(HERE / file, qa / file)
    (meta / "build-manifest.json").write_bytes(raw)
    install(BWRAP, root / "bwrap", True)
    need(file_sha(root / "bwrap") == host["bwrap_sha256"], "bwrap_copy_pin")
    runtime_pins = {str(p): file_sha(p) for p in elf_dependencies(BWRAP)}
    lock = {"kind": "corral_component_qa_only", "scenarios": opts.scenario or SCENARIOS, "manifest_sha256": sha(raw),
            "profile_sha256": PROFILE_SHA, "packet_sha256": PACKET_SHA, "host": host,
            "bwrap_dependencies": runtime_pins, "qa": {file: file_sha(qa / file) for file in QA_FILES}}
    pins = {"profile_sha256": PROFILE_SHA, "packet_sha256": PACKET_SHA, "manifest_sha256": sha(raw),
            "bwrap_sha256": host["bwrap_sha256"],
            "components": {role: components[path]["sha256"] for role, path in ENTRYPOINTS.items()}}
    if opts.bundle:
        pins["archive_sha256"] = lock["archive_sha256"] = opts.archive_sha256
    (meta / "qa-lock.json").write_bytes(wire(lock))
    pins["qa_lock_sha256"] = sha(wire(lock))
    (root / "state").mkdir(mode=0o700)
    (root / "scratch").mkdir(mode=0o700)
    return pins
