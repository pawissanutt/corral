#!/bin/sh
# Deterministic installer integration tests. No network, real engine, or live session.
# Run: sh scripts/install-test.sh </dev/null
set -eu

installer=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)/install.sh
original_path=$PATH
suite=$(mktemp -d)
first=
cleanup() {
    status=$?
    if [ -n "$first" ]; then
        : >"$TEST_ROOT/continue"
        wait "$first" || true
    fi
    if [ "$status" -ne 0 ] && [ -n "${TEST_ROOT:-}" ] && [ -f "$TEST_ROOT/output" ]; then
        PATH=$original_path grep "^" "$TEST_ROOT/output" >&2
    fi
    rm -rf "$suite"
}
trap cleanup EXIT
mkdir -p "$suite/bin" "$suite/fixtures"

# Restrict PATH so an installed herdr (or real downloader) cannot be reached.
export TEST_REAL_MV
TEST_REAL_MV=$(command -v mv)
for cmd in sh awk mktemp rm tar gzip mkdir rmdir cp chmod cmp ln sha256sum script sleep; do
    ln -s "$(command -v "$cmd")" "$suite/bin/$cmd"
done
cat >"$suite/bin/mv" <<'SH'
#!/bin/sh
set -eu
case "$1:$2" in
-f:*/new/corrald-next)
    [ "${TEST_FAIL_PROMOTION:-0}" = 0 ] || exit 47
    ;;
*/.corral-app-install.*/new/site:*)
    if [ "${TEST_PROMOTION_COLLISION:-0}" = 1 ]; then
        mkdir "$CORRAL_APP_INSTALL_DIR"
        printf 'unrelated\n' >"$CORRAL_APP_INSTALL_DIR/index.html"
    fi
    ;;
esac
"$TEST_REAL_MV" "$@"
if [ "$1" = "$CORRAL_APP_INSTALL_DIR" ] && [ "${TEST_PAUSE_BACKUP:-0}" = 1 ]; then
    # The live site is now absent, but the first installer still owns the locks.
    : >"$TEST_ROOT/paused"
    tries=0
    until [ -f "$TEST_ROOT/continue" ]; do
        [ "$tries" -lt 300 ] || exit 48
        tries=$((tries + 1))
        sleep 0.05
    done
fi
SH
chmod +x "$suite/bin/mv"
cat >"$suite/bin/uname" <<'SH'
#!/bin/sh
case "$1" in
-s) echo "${TEST_OS:-Linux}" ;;
-m) echo "${TEST_ARCH:-x86_64}" ;;
*) exit 1 ;;
esac
SH
cat >"$suite/bin/curl" <<'SH'
#!/bin/sh
set -eu
case "$1" in
-fsSL) url=$2; destination=$4 ;;
-qO) destination=$2; url=$3 ;;
*) exit 1 ;;
esac
printf '%s\n' "$url" >>"$TEST_ROOT/downloads"
case "$url" in
https://herdr.dev/install.sh)
    [ "${TEST_ENGINE_DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
    cp "$TEST_FIXTURES/engine.sh" "$destination"
    ;;
https://fixture.invalid/*/*.tar.gz.sha256)
    [ "${TEST_CHECKSUM_DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
    if [ "${TEST_BAD_CHECKSUM:-0}" = 1 ]; then
        printf '%064d  corral-x86_64-unknown-linux-musl.tar.gz\n' 0 >"$destination"
    elif [ "${TEST_WRONG_CHECKSUM_NAME:-0}" = 1 ]; then
        printf '%064d  unrelated.tar.gz\n' 0 >"$destination"
    else
        case "${url##*/}" in
        corral-next-* | corral-app-web.*)
            cp "$TEST_FIXTURES/${TEST_CURRENT_FIXTURE:-1}/${url##*/}" "$destination" ;;
        *) cp "$TEST_FIXTURES/${TEST_ARCHIVE:-release}.sha256" "$destination" ;;
        esac
    fi
    ;;
https://fixture.invalid/*/*.tar.gz)
    [ "${TEST_RELEASE_DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
    case "${url##*/}" in
    corral-next-* | corral-app-web.*)
        cp "$TEST_FIXTURES/${TEST_CURRENT_FIXTURE:-1}/${url##*/}" "$destination" ;;
    *) cp "$TEST_FIXTURES/${TEST_ARCHIVE:-release}.tar.gz" "$destination" ;;
    esac
    ;;
*) exit 23 ;;
esac
SH
cat >"$suite/fixtures/herdr" <<'SH'
#!/bin/sh
set -eu
# Reject every operation except version discovery.
[ "$#" = 1 ] && [ "$1" = --version ] || exit 99
printf '%s\n' "$*" >>"$TEST_ROOT/herdr-calls"
echo "${TEST_HERDR_VERSION:-herdr 0.9.3}"
exit "${TEST_VERSION_EXIT:-0}"
SH
cat >"$suite/fixtures/corral" <<'SH'
#!/bin/sh
case "$*" in
--version)
    echo "corral ${TEST_CORRAL_VERSION:-fixture}"
    exit "${TEST_BINARY_EXIT:-0}"
    ;;
"agent install")
    # The real command installs the sibling corral-pi-host; record that it ran beside one.
    [ -x "${0%/*}/corral-pi-host" ] || exit 98
    printf 'agent install\n' >>"$TEST_ROOT/agent-calls"
    exit "${TEST_AGENT_INSTALL_EXIT:-0}"
    ;;
*) exit 99 ;;
esac
SH
cat >"$suite/fixtures/corral-pi-host" <<'SH'
#!/bin/sh
[ "$#" = 1 ] && [ "$1" = --version ] || exit 99
echo "corral-pi-host pi fixture bun fixture"
exit "${TEST_HOST_EXIT:-0}"
SH
cat >"$suite/fixtures/engine.sh" <<'SH'
#!/bin/sh
printf 'accepted\n' >"$TEST_ROOT/engine-ran"
exit "${TEST_ENGINE_EXIT:-0}"
SH
chmod +x "$suite/bin/uname" "$suite/bin/curl" "$suite/fixtures/herdr" "$suite/fixtures/corral" \
    "$suite/fixtures/corral-pi-host"
# release: corral with its compiled corral-pi host (0.4.0+); legacy: corral alone (earlier).
tar -czf "$suite/fixtures/release.tar.gz" -C "$suite/fixtures" corral corral-pi-host
tar -czf "$suite/fixtures/legacy.tar.gz" -C "$suite/fixtures" corral
for archive in release legacy; do
    hash=$(sha256sum "$suite/fixtures/$archive.tar.gz")
    printf '%s  corral-x86_64-unknown-linux-musl.tar.gz\n' "${hash%% *}" >"$suite/fixtures/$archive.sha256"
done

export PATH="$suite/bin" TEST_FIXTURES="$suite/fixtures" SHELL=/bin/sh
cases=0
new_case() {
    cases=$((cases + 1))
    export TEST_ROOT="$suite/case-$cases"
    export HOME="$TEST_ROOT/home" CORRAL_INSTALL_DIR="$TEST_ROOT/install"
    export CORRAL_APP_INSTALL_DIR="$TEST_ROOT/site"
    export CORRAL_BASE_URL=https://fixture.invalid/releases CORRAL_VERSION=latest
    unset TEST_CURRENT_FIXTURE TEST_FAIL_PROMOTION TEST_PAUSE_BACKUP TEST_PROMOTION_COLLISION
    unset TEST_HERDR_VERSION TEST_VERSION_EXIT TEST_ENGINE_DOWNLOAD_FAIL TEST_ENGINE_EXIT
    unset TEST_BAD_CHECKSUM TEST_ARCH TEST_OS CORRAL_REPO
    unset TEST_CORRAL_VERSION TEST_BINARY_EXIT TEST_WRONG_CHECKSUM_NAME
    unset TEST_RELEASE_DOWNLOAD_FAIL TEST_CHECKSUM_DOWNLOAD_FAIL
    unset TEST_ARCHIVE TEST_HOST_EXIT TEST_AGENT_INSTALL_EXIT
    mkdir -p "$HOME"
    rm -f "$suite/bin/herdr" "$suite/bin/wget"
    [ -f "$suite/bin/curl" ] || cp "$suite/fixtures/downloader" "$suite/bin/curl"
}
cp "$suite/bin/curl" "$suite/fixtures/downloader"
run() {
    sh "$installer" "$@" >"$TEST_ROOT/output" 2>&1 </dev/null
}
installed() {
    [ -x "$CORRAL_INSTALL_DIR/corral" ]
    [ "$("$CORRAL_INSTALL_DIR/corral" --version)" = "corral ${TEST_CORRAL_VERSION:-fixture}" ]
}
host_installed() {
    [ -x "$CORRAL_INSTALL_DIR/corral-pi-host" ]
    [ "$(PATH=$original_path wc -l <"$TEST_ROOT/agent-calls")" -eq 1 ]
}
engine_skipped() { [ ! -f "$TEST_ROOT/engine-ran" ]; }
engine_accepted() { [ -f "$TEST_ROOT/engine-ran" ]; }

export CORRAL_INSTALL_LEGACY=1
# Given no engine and noninteractive stdin, installation continues without consent.
new_case
run
installed
host_installed
engine_skipped

# Given a release archive from before the compiled host, corral installs alone.
new_case
export TEST_ARCHIVE=legacy
run
installed
[ ! -e "$CORRAL_INSTALL_DIR/corral-pi-host" ]
[ ! -e "$TEST_ROOT/agent-calls" ]

# Given a failing 'corral agent install', corral stays installed and the note names the command.
new_case
export TEST_AGENT_INSTALL_EXIT=1
run
installed
host_installed
PATH=$original_path grep -F "note: 'corral agent install' failed" "$TEST_ROOT/output" >/dev/null
PATH=$original_path grep -F 'curl -fsSL https://herdr.dev/install.sh | sh' "$TEST_ROOT/output" >/dev/null

# Given a missing engine, --yes explicitly authorizes only the fixture installer.
new_case
run --yes
installed
engine_accepted

# Given older, equal, newer, prerelease, or unreadable versions, compare semantically.
for spec in '0.9.2 yes' '0.9.3 no' '0.9.4 no' '0.10.0 no' '1.0.0 no' \
    '0.8.99 yes' '0.9.3-rc.1 yes' 'v0.9.3 no' '0.9.3+build-test no' 'unknown yes'; do
    new_case
    version=${spec% *}
    expected=${spec##* }
    ln -s "$suite/fixtures/herdr" "$suite/bin/herdr"
    export TEST_HERDR_VERSION="herdr $version"
    run --yes
    installed
    case "$expected" in
    yes) engine_accepted ;;
    no) engine_skipped ;;
    esac
    [ "$(PATH=$original_path wc -l <"$TEST_ROOT/herdr-calls")" -eq 1 ]
done

# Given an old engine, no --yes still skips without blocking or touching sessions.
new_case
ln -s "$suite/fixtures/herdr" "$suite/bin/herdr"
export TEST_HERDR_VERSION='herdr 0.9.1'
run
installed
engine_skipped

# Given a failed version probe, treat the engine as unavailable.
new_case
ln -s "$suite/fixtures/herdr" "$suite/bin/herdr"
export TEST_VERSION_EXIT=1 TEST_HERDR_VERSION=''
run --yes
installed
engine_accepted

# Given terminal stdin, drive consent through a private PTY, never a live session.
for answer in yes no; do
    new_case
    printf '%s\n' "$answer" | script -q -e -c "sh '$installer'" /dev/null >"$TEST_ROOT/output" 2>&1
    installed
    case "$answer" in
    yes) engine_accepted ;;
    no) engine_skipped ;;
    esac
done

# Given dry-run in either option order, neither downloader nor installer runs.
for flags in '--dry-run --yes' '--yes --dry-run'; do
    new_case
    # Intentional splitting: these are two fixed option lists, not user input.
    # shellcheck disable=SC2086
    run $flags
    engine_skipped
    [ ! -e "$TEST_ROOT/downloads" ]
    [ ! -e "$CORRAL_INSTALL_DIR" ]
done

# Given explicit corral download overrides, preserve the version, base and destination.
new_case
export CORRAL_VERSION=v7.8.9 TEST_CORRAL_VERSION=7.8.9 CORRAL_INSTALL_DIR="$HOME/custom bin"
run
installed
PATH=$original_path grep -Fx 'https://fixture.invalid/releases/download/v7.8.9/corral-x86_64-unknown-linux-musl.tar.gz' "$TEST_ROOT/downloads" >/dev/null

# Given only a repository override, dry-run constructs the corresponding GitHub URL.
new_case
unset CORRAL_BASE_URL
export CORRAL_REPO=example/custom CORRAL_VERSION=v2.0.0
run --dry-run
PATH=$original_path grep -F 'https://github.com/example/custom/releases/download/v2.0.0/' "$TEST_ROOT/output" >/dev/null
[ ! -e "$TEST_ROOT/downloads" ]

# Given wget only, both engine bootstrap and verified corral installation still work.
new_case
mv "$suite/bin/curl" "$suite/bin/wget"
run --yes
installed
engine_accepted

# Given failed downloads or installers, stop with failure rather than report success.
for failure in download engine checksum; do
    new_case
    case "$failure" in
    download) export TEST_ENGINE_DOWNLOAD_FAIL=1 ;;
    engine) export TEST_ENGINE_EXIT=1 ;;
    checksum) export TEST_BAD_CHECKSUM=1 ;;
    esac
    if run --yes; then
        echo "unexpected success: $failure" >&2
        exit 1
    fi
    [ ! -e "$CORRAL_INSTALL_DIR/corral" ]
done

# Given an unknown option or unsupported platform, fail without downloading.
for option in --unknown unsupported-os unsupported-cpu; do
    new_case
    case "$option" in
    unsupported-os) export TEST_OS=Windows; set -- ;;
    unsupported-cpu) export TEST_ARCH=riscv64; set -- ;;
    *) set -- "$option" ;;
    esac
    if run "$@"; then
        echo "unexpected success: $option" >&2
        exit 1
    fi
    [ ! -e "$TEST_ROOT/downloads" ]
done

# Given an existing installation, failed updates leave its bytes unchanged.
for failure in archive-download checksum-download checksum manifest binary host version; do
    new_case
    run
    prior=$(sha256sum "$CORRAL_INSTALL_DIR/corral")
    prior_host=$(sha256sum "$CORRAL_INSTALL_DIR/corral-pi-host")
    case "$failure" in
    archive-download) export TEST_RELEASE_DOWNLOAD_FAIL=1 ;;
    checksum-download) export TEST_CHECKSUM_DOWNLOAD_FAIL=1 ;;
    checksum) export TEST_BAD_CHECKSUM=1 ;;
    manifest) export TEST_WRONG_CHECKSUM_NAME=1 ;;
    binary) export TEST_BINARY_EXIT=1 ;;
    host) export TEST_HOST_EXIT=1 ;;
    version) export CORRAL_VERSION=v9.8.7 ;;
    esac
    if run; then
        echo "unexpected update success: $failure" >&2
        exit 1
    fi
    [ "$(sha256sum "$CORRAL_INSTALL_DIR/corral")" = "$prior" ]
    [ "$(sha256sum "$CORRAL_INSTALL_DIR/corral-pi-host")" = "$prior_host" ]
    set -- "$CORRAL_INSTALL_DIR"/.corral-install.* "$CORRAL_INSTALL_DIR"/.corral-pi-host-install.*
    [ ! -e "$1" ] && [ ! -e "$2" ]
done

# Given a release path instead of a tag, reject it before any network request.
new_case
export CORRAL_VERSION=v1.0.0/../../other
if run; then
    echo "unexpected success: invalid version" >&2
    exit 1
fi
[ ! -e "$TEST_ROOT/downloads" ]

unset CORRAL_INSTALL_LEGACY
for revision in 1 2; do
    fixture=$TEST_FIXTURES/$revision
    mkdir "$fixture"
    printf 'site %s\n' "$revision" >"$fixture/index.html"
    for spec in 'corral-next|corral-next' 'corrald-next|corral-daemon' 'corral-pi|corral-pi pi'; do
        name=${spec%%|*}
        # The fixture, not this harness, expands $1.
        # shellcheck disable=SC2016
        printf '#!/bin/sh\n[ "$1" = --version ] || exit 98\necho "%s 7.8.%s"\n' "${spec#*|}" "$revision" >"$fixture/$name"
    done
    tar -czf "$fixture/corral-app-web.tar.gz" -C "$fixture" index.html
    tar -czf "$fixture/corral-next-x86_64-unknown-linux-gnu.tar.gz" -C "$fixture" corral-next corrald-next corral-pi
    (cd "$fixture" && sha256sum corral-app-web.tar.gz >corral-app-web.tar.gz.sha256 &&
        sha256sum corral-next-x86_64-unknown-linux-gnu.tar.gz >corral-next-x86_64-unknown-linux-gnu.tar.gz.sha256)
done

# Ordinary sites and hidden entries (including dangling links) are not ownership proof.
for existing in index dotfile marker directory symlink; do
    new_case
    mkdir "$CORRAL_APP_INSTALL_DIR"
    case "$existing" in
    index) echo unrelated >"$CORRAL_APP_INSTALL_DIR/index.html"; entry=index.html ;;
    dotfile) ln -s nowhere "$CORRAL_APP_INSTALL_DIR/.user-file"; entry=.user-file ;;
    marker) echo unrelated >"$CORRAL_APP_INSTALL_DIR/.corral-web-install"; entry=.corral-web-install ;;
    directory) mkdir "$CORRAL_APP_INSTALL_DIR/.corral-web-install"; entry=.corral-web-install ;;
    symlink)
        echo corral-web-install-v1 >"$TEST_ROOT/fake-marker"
        ln -s "$TEST_ROOT/fake-marker" "$CORRAL_APP_INSTALL_DIR/.corral-web-install"
        entry=.corral-web-install
        ;;
    esac
    if run --app-only; then echo "unexpected adoption: $existing" >&2; exit 1; fi
    PATH=$original_path grep -F 'destination is nonempty and not managed' "$TEST_ROOT/output" >/dev/null
    [ -e "$CORRAL_APP_INSTALL_DIR/$entry" ] || [ -L "$CORRAL_APP_INSTALL_DIR/$entry" ]
    case "$existing" in
    index | marker) printf 'unrelated\n' | cmp -s - "$CORRAL_APP_INSTALL_DIR/$entry" ;;
    esac
done

verify_current() {
    printf 'corral-web-install-v1\n' | cmp -s - "$CORRAL_APP_INSTALL_DIR/.corral-web-install"
    printf 'site %s\n' "$1" | cmp -s - "$CORRAL_APP_INSTALL_DIR/index.html"
    [ "$("$CORRAL_INSTALL_DIR/corral-next" --version)" = "corral-next 7.8.$1" ]
    [ "$("$CORRAL_INSTALL_DIR/corrald-next" --version)" = "corral-daemon 7.8.$1" ]
    [ "$("$CORRAL_INSTALL_DIR/corral-pi" --version)" = "corral-pi pi 7.8.$1" ]
    [ ! -e "$CORRAL_INSTALL_DIR/.corral-next-install.lock" ]
    [ ! -e "$TEST_ROOT/.corral-web-install.lock" ]
}

# The installer can adopt an empty directory and later replace its own site.
new_case
mkdir "$CORRAL_APP_INSTALL_DIR"
run
verify_current 1
: >"$CORRAL_APP_INSTALL_DIR/obsolete"
TEST_CURRENT_FIXTURE=2 CORRAL_VERSION=v7.8.2 run
verify_current 2
[ ! -e "$CORRAL_APP_INSTALL_DIR/obsolete" ]

# A promotion failure after the web site and CLI were exchanged restores the old
# release, or the absence of those files on a fresh install.
for prior in existing absent; do
    new_case
    if [ "$prior" = existing ]; then
        run
        printf 'old-only data\n' >"$CORRAL_APP_INSTALL_DIR/previous"
    fi
    if TEST_CURRENT_FIXTURE=2 TEST_FAIL_PROMOTION=1 run; then
        echo "unexpected success after injected promotion failure" >&2
        exit 1
    fi
    if [ "$prior" = existing ]; then
        verify_current 1
        printf 'old-only data\n' | cmp -s - "$CORRAL_APP_INSTALL_DIR/previous"
    else
        [ ! -e "$CORRAL_APP_INSTALL_DIR" ]
        for name in corral-next corrald-next corral-pi .corral-next-install.lock; do
            [ ! -e "$CORRAL_INSTALL_DIR/$name" ]
        done
        [ ! -e "$TEST_ROOT/.corral-web-install.lock" ]
    fi
done

# An unrelated writer is not bound by the lock. If it recreates the destination
# after backup, do not replace it or throw away the old installation.
new_case
run --app-only
if TEST_CURRENT_FIXTURE=2 TEST_PROMOTION_COLLISION=1 run --app-only; then
    echo 'unexpected success after an unrelated change to the destination' >&2
    exit 1
fi
PATH=$original_path grep -F 'could not roll back' "$TEST_ROOT/output" >/dev/null
printf 'unrelated\n' | cmp -s - "$CORRAL_APP_INSTALL_DIR/index.html"
set -- "$TEST_ROOT"/.corral-app-install.*/old/site/index.html
[ "$#" -eq 1 ]
printf 'site 1\n' | cmp -s - "$1"
[ ! -e "$TEST_ROOT/.corral-web-install.lock" ]

# Reproduce the old window precisely, from a second process while the first is
# paused after moving the old site aside. Both shared destinations must be locked.
new_case
run
TEST_CURRENT_FIXTURE=2 TEST_PAUSE_BACKUP=1 sh "$installer" >"$TEST_ROOT/first-output" 2>&1 </dev/null &
first=$!
tries=0
until [ -f "$TEST_ROOT/paused" ]; do
    if ! kill -0 "$first" 2>/dev/null || [ "$tries" -ge 200 ]; then
        PATH=$original_path cat "$TEST_ROOT/first-output" >&2
        exit 1
    fi
    tries=$((tries + 1))
    sleep 0.05
done
[ ! -e "$CORRAL_APP_INSTALL_DIR" ]
if run --app-only; then echo 'concurrent app-only install unexpectedly succeeded' >&2; exit 1; fi
PATH=$original_path grep -F 'cannot acquire install lock' "$TEST_ROOT/output" >/dev/null
mkdir "$TEST_ROOT/other-parent"
if CORRAL_APP_INSTALL_DIR="$TEST_ROOT/other-parent/site" run; then
    echo 'concurrent install sharing bin directory unexpectedly succeeded' >&2
    exit 1
fi
PATH=$original_path grep -F '.corral-next-install.lock' "$TEST_ROOT/output" >/dev/null
: >"$TEST_ROOT/continue"
if ! wait "$first"; then PATH=$original_path cat "$TEST_ROOT/first-output" >&2; exit 1; fi
first=
verify_current 2
[ ! -e "$CORRAL_APP_INSTALL_DIR/site" ]
[ ! -e "$TEST_ROOT/other-parent/site" ]

printf 'installer tests passed: %s isolated cases\n' "$cases"
