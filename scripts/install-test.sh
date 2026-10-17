#!/bin/sh
# Deterministic installer integration tests. No network, real engine, or live session.
# Run: sh scripts/install-test.sh </dev/null
set -eu

installer=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)/install.sh
original_path=$PATH
suite=$(mktemp -d)
cleanup() {
    status=$?
    if [ "$status" -ne 0 ] && [ -n "${TEST_ROOT:-}" ] && [ -f "$TEST_ROOT/output" ]; then
        PATH=$original_path grep "^" "$TEST_ROOT/output" >&2
    fi
    rm -rf "$suite"
}
trap cleanup EXIT
mkdir -p "$suite/bin" "$suite/fixtures"

# Restrict PATH so an installed herdr (or real downloader) cannot be reached.
for cmd in sh awk mktemp rm tar gzip mkdir cp chmod mv ln sha256sum script; do
    ln -s "$(command -v "$cmd")" "$suite/bin/$cmd"
done
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
        cp "$TEST_FIXTURES/release.sha256" "$destination"
    fi
    ;;
https://fixture.invalid/*/*.tar.gz)
    [ "${TEST_RELEASE_DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
    cp "$TEST_FIXTURES/release.tar.gz" "$destination"
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
[ "$#" = 1 ] && [ "$1" = --version ] || exit 99
echo "corral ${TEST_CORRAL_VERSION:-fixture}"
exit "${TEST_BINARY_EXIT:-0}"
SH
cat >"$suite/fixtures/engine.sh" <<'SH'
#!/bin/sh
printf 'accepted\n' >"$TEST_ROOT/engine-ran"
exit "${TEST_ENGINE_EXIT:-0}"
SH
chmod +x "$suite/bin/uname" "$suite/bin/curl" "$suite/fixtures/herdr" "$suite/fixtures/corral"
tar -czf "$suite/fixtures/release.tar.gz" -C "$suite/fixtures" corral
hash=$(sha256sum "$suite/fixtures/release.tar.gz")
printf '%s  corral-x86_64-unknown-linux-musl.tar.gz\n' "${hash%% *}" >"$suite/fixtures/release.sha256"

export PATH="$suite/bin" TEST_FIXTURES="$suite/fixtures" SHELL=/bin/sh
cases=0
new_case() {
    cases=$((cases + 1))
    export TEST_ROOT="$suite/case-$cases"
    export HOME="$TEST_ROOT/home" CORRAL_INSTALL_DIR="$TEST_ROOT/install"
    export CORRAL_BASE_URL=https://fixture.invalid/releases CORRAL_VERSION=latest
    unset TEST_HERDR_VERSION TEST_VERSION_EXIT TEST_ENGINE_DOWNLOAD_FAIL TEST_ENGINE_EXIT
    unset TEST_BAD_CHECKSUM TEST_ARCH TEST_OS CORRAL_REPO
    unset TEST_CORRAL_VERSION TEST_BINARY_EXIT TEST_WRONG_CHECKSUM_NAME
    unset TEST_RELEASE_DOWNLOAD_FAIL TEST_CHECKSUM_DOWNLOAD_FAIL
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
engine_skipped() { [ ! -f "$TEST_ROOT/engine-ran" ]; }
engine_accepted() { [ -f "$TEST_ROOT/engine-ran" ]; }

# Given no engine and noninteractive stdin, installation continues without consent.
new_case
run
installed
engine_skipped
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
for failure in archive-download checksum-download checksum manifest binary version; do
    new_case
    run
    prior=$(sha256sum "$CORRAL_INSTALL_DIR/corral")
    case "$failure" in
    archive-download) export TEST_RELEASE_DOWNLOAD_FAIL=1 ;;
    checksum-download) export TEST_CHECKSUM_DOWNLOAD_FAIL=1 ;;
    checksum) export TEST_BAD_CHECKSUM=1 ;;
    manifest) export TEST_WRONG_CHECKSUM_NAME=1 ;;
    binary) export TEST_BINARY_EXIT=1 ;;
    version) export CORRAL_VERSION=v9.8.7 ;;
    esac
    if run; then
        echo "unexpected update success: $failure" >&2
        exit 1
    fi
    [ "$(sha256sum "$CORRAL_INSTALL_DIR/corral")" = "$prior" ]
    set -- "$CORRAL_INSTALL_DIR"/.corral-install.*
    [ ! -e "$1" ]
done

# Given a release path instead of a tag, reject it before any network request.
new_case
export CORRAL_VERSION=v1.0.0/../../other
if run; then
    echo "unexpected success: invalid version" >&2
    exit 1
fi
[ ! -e "$TEST_ROOT/downloads" ]

printf 'installer tests passed: %s isolated cases\n' "$cases"
