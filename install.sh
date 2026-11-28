#!/bin/sh
set -eu

repo="${CORRAL_REPO:-pawissanutt/corral}"
version="${CORRAL_VERSION:-latest}"
base="${CORRAL_BASE_URL:-https://github.com/$repo/releases}"
base=${base%/}
bin_dir="${CORRAL_INSTALL_DIR:-$HOME/.local/bin}"
app_dir="${CORRAL_APP_INSTALL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/corral-next/web}"
app_asset=corral-app-web.tar.gz
web_marker=.corral-web-install
web_marker_text=corral-web-install-v1
# Keep in sync with the plugin's min_herdr_version.
min_herdr_version=0.9.3
herdr_installer=https://herdr.dev/install.sh
yes=false
dry_run=false
app_only=false

fail() {
    echo "corral: $*" >&2
    exit 1
}

retired_classic() {
    fail "classic installation is retired; see the v0.4.0 archive: https://github.com/pawissanutt/corral/tree/v0.4.0"
}

[ "${CORRAL_INSTALL_LEGACY:-0}" != 1 ] || retired_classic

for arg in "$@"; do
    case "$arg" in
    --yes) yes=true ;;
    --dry-run) dry_run=true ;;
    --legacy) retired_classic ;;
    --app-only) app_only=true ;;
    *) fail "unknown option: $arg (use --yes, --dry-run or --app-only)" ;;
    esac
done

valid_tag() {
    printf '%s\n' "$1" | LC_ALL=C awk '
        NR == 1 && /^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?([+][0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/ { valid=1 }
        END { exit !(NR == 1 && valid) }
    '
}
[ "$version" = latest ] || valid_tag "$version" || fail "invalid release tag: $version (expected latest or vX.Y.Z)"

if [ "$app_only" = false ]; then
    case "$(uname -s)" in
    Linux) os=unknown-linux-gnu ;;
    Darwin) os=apple-darwin ;;
    *) fail "unsupported OS $(uname -s); on native Windows use install.ps1" ;;
    esac
    case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "unsupported CPU $(uname -m)" ;;
    esac
    asset="corral-next-$arch-$os.tar.gz"
fi

release_url() {
    if [ "$1" = latest ]; then
        printf '%s/latest/download/%s\n' "$base" "$2"
    else
        printf '%s/download/%s/%s\n' "$base" "$1" "$2"
    fi
}

if [ "$dry_run" = true ]; then
    if [ "$app_only" = false ]; then
        echo "would download and verify $(release_url "$version" "$asset") into $bin_dir"
        echo "would install corral-next, corrald-next and corral-pi (no daemon changes)"
        echo "would check for herdr >= $min_herdr_version, corral's terminal engine"
        echo "if needed, would offer: curl -fsSL $herdr_installer | sh"
    fi
    app_tag=$version
    if [ "$app_only" = false ] && [ "$version" = latest ]; then app_tag='<runtime tag>'; fi
    echo "would download and verify $(release_url "$app_tag" "$app_asset") into $app_dir (no web server started)"
    exit 0
fi

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2" --connect-timeout 15 --max-time 120
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1" --timeout=120
    else
        fail "need curl or wget"
    fi
}

# Verify one checksum for this archive only; do not pass other filenames to -c.
download() {
    url=$(release_url "$1" "$2")
    if ! fetch "$url" "$tmp/$2"; then
        fail "download failed: $url"
    fi
    fetch "$url.sha256" "$tmp/$2.sha256" || fail "checksum download failed: $url.sha256"
    LC_ALL=C awk -v asset="$2" '
        NR == 1 && length($1) == 64 && $1 !~ /[^0-9a-fA-F]/ && ($2 == asset || $2 == "*" asset) && NF == 2 { valid=1 }
        END { exit !(NR == 1 && valid) }
    ' "$tmp/$2.sha256" || fail "invalid checksum manifest for $2"
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$tmp" && sha256sum -c "$2.sha256" >/dev/null) || fail "checksum mismatch for $2"
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$tmp" && shasum -a 256 -c "$2.sha256" >/dev/null) || fail "checksum mismatch for $2"
    else
        fail "need sha256sum or shasum to verify the download"
    fi
}

# Release tarballs contain ordinary files/directories only, with paths local to the archive.
unpack() {
    LC_ALL=C tar -tzf "$tmp/$1" >"$tmp/entries" || fail "invalid archive: $1"
    LC_ALL=C awk '
        { sub(/^\.\//, ""); sub(/\/$/, "") }
        $0 == "" || $0 == "." { next }
        /^\/|[[:cntrl:]\\]/ || /(^|\/)\.\.?($|\/)|\/\// { exit 1 }
    ' "$tmp/entries" || fail "unsafe archive paths: $1"
    LC_ALL=C tar -tvzf "$tmp/$1" >"$tmp/entries" || fail "invalid archive: $1"
    LC_ALL=C awk '/^[^d-]/ { exit 1 }' "$tmp/entries" || fail "archive contains links or special files: $1"
    tar -xzf "$tmp/$1" -C "$2" || fail "invalid archive: $1"
}

regular_file() { [ -f "$1" ] && [ ! -L "$1" ]; }
path_exists() { [ -e "$1" ] || [ -L "$1" ]; }
same_file() {
    # Supported by the Linux and macOS shells (and specified in POSIX.1-2024).
    # shellcheck disable=SC3013
    [ "$1" -ef "$2" ]
}

# Atomic even on macOS (which does not ship flock). Never remove a lock we did
# not acquire. An unclean shutdown may leave a lock; fail closed in that case.
lock_install() {
    mkdir -m 700 "$1" || fail "cannot acquire install lock: $1 (another install may be running; remove a stale lock only after checking the destination)"
}

names='corral-next corrald-next corral-pi'
tmp=$(mktemp -d)
bin_txn=
bin_lock=
web_txn=
web_lock=
committing=false

rollback_bins() {
    for name in $names; do
        # A missing staged entry was promoted. Restore the old copy (or absence).
        path_exists "$bin_txn/new/$name" && continue
        if ! path_exists "$bin_dir/$name" || regular_file "$bin_dir/$name"; then
            if path_exists "$bin_txn/old/$name"; then
                mv -f "$bin_txn/old/$name" "$bin_dir" && continue
            else
                rm -f "$bin_dir/$name" && continue
            fi
        fi
        echo "corral: could not roll back $bin_dir/$name; inspect $bin_txn/old" >&2
        keep_bin=true
    done
}

rollback_web() {
    # Remove only the site from this transaction. Never recursively delete the
    # destination or discard a backup when an unexpected path blocks rollback.
    if ! path_exists "$web_site" && path_exists "$app_dir"; then
        if [ ! -L "$app_dir" ] && regular_file "$app_dir/$web_marker" &&
            same_file "$app_dir/$web_marker" "$web_txn/staged-marker"; then
            mv "$app_dir" "$web_txn/new" || keep_web=true
        else
            keep_web=true
        fi
    fi
    if path_exists "$web_backup"; then
        # mv has no portable atomic no-replace mode; -n is not a guarantee.
        # Retain directory backups for manual recovery. For other objects,
        # ln -P without -f atomically creates the same basename in the parent,
        # or fails without replacing an arrival (even a directory or symlink).
        if { [ ! -L "$web_backup" ] && [ -d "$web_backup" ]; } ||
            ! ln -P "$web_backup" "$app_parent"; then
            keep_web=true
        fi
    fi
    if [ "${keep_web:-false}" = true ]; then
        echo "corral: could not roll back $app_dir; inspect $web_txn (previous site backup: $web_backup)" >&2
    fi
}

cleanup() {
    trap '' HUP INT TERM
    if [ "$committing" = true ]; then
        if [ -n "$bin_txn" ]; then rollback_bins; fi
        rollback_web
    fi
    if [ -n "$bin_txn" ] && [ "${keep_bin:-false}" = false ]; then rm -rf "$bin_txn" || true; fi
    if [ -n "$web_txn" ] && [ "${keep_web:-false}" = false ]; then rm -rf "$web_txn" || true; fi
    if [ -n "$web_lock" ]; then rmdir "$web_lock" || true; fi
    if [ -n "$bin_lock" ]; then rmdir "$bin_lock" || true; fi
    rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# Compare numeric components, not strings (0.10.0 is newer than 0.9.3).
herdr_compatible() {
    awk -v minimum="$min_herdr_version" '
        {
            for (i = 1; i <= NF; i++) {
                if ($i ~ /^v?[0-9]+\.[0-9]+\.[0-9]+([-+][^ ]+)?$/) {
                    version = $i
                    sub(/^v/, "", version)
                    sub(/\+.*/, "", version)
                    prerelease = version ~ /-/
                    sub(/[-+].*$/, "", version)
                    split(version, actual, ".")
                    split(minimum, required, ".")
                    for (j = 1; j <= 3; j++) {
                        if (actual[j] + 0 > required[j] + 0) exit 0
                        if (actual[j] + 0 < required[j] + 0) exit 1
                    }
                    exit prerelease
                }
            }
            exit 1
        }
        END { if (NR == 0) exit 1 }
    '
}

check_herdr() {
    herdr_version=$(herdr --version 2>/dev/null) || herdr_version=
    if printf '%s\n' "$herdr_version" | herdr_compatible; then return; fi

    echo "corral uses herdr as its terminal engine to host the board and agent panes."
    echo "herdr >= $min_herdr_version is required; found: ${herdr_version:-not installed}."
    echo "Official install/upgrade command: curl -fsSL $herdr_installer | sh"
    consent="$yes"
    if [ "$consent" = true ]; then
        echo "--yes accepts installing/upgrading the herdr terminal engine."
    elif [ -t 0 ]; then
        printf 'Install/upgrade herdr now? [y/N] '
        answer=
        read -r answer || true
        case "$answer" in
        y | Y | yes | YES) consent=true ;;
        esac
    fi
    if [ "$consent" = true ]; then
        fetch "$herdr_installer" "$tmp/herdr-install.sh" || fail "herdr installer download failed"
        sh "$tmp/herdr-install.sh" || fail "herdr installation failed"
    else
        echo "Skipping herdr installation; continuing with corral. Install herdr before starting a workspace."
    fi
}

check_pin() {
    if [ "$version" != latest ] && [ "$2" != "$1 ${version#v}" ]; then
        fail "version mismatch: requested $version, got $2; existing install unchanged"
    fi
}

prepare_runtime() {
    download "$version" "$asset"
    mkdir -p "$bin_dir"
    bin_dir=$(CDPATH='' cd -- "$bin_dir" && pwd -P)
    bin_txn=$(mktemp -d "$bin_dir/.corral-next-install.XXXXXX")
    mkdir "$bin_txn/new" "$bin_txn/old"
    unpack "$asset" "$bin_txn/new"
    for name in $names; do
        regular_file "$bin_txn/new/$name" || fail "archive has no regular $name binary"
        chmod 755 "$bin_txn/new/$name"
    done
    cli=$("$bin_txn/new/corral-next" --version </dev/null) || fail "corral-next cannot run on this machine; existing install unchanged"
    case "$cli" in
    'corral-next '*) runtime_tag=v${cli#corral-next } ;;
    *) fail "downloaded CLI did not identify itself as corral-next; existing install unchanged" ;;
    esac
    valid_tag "$runtime_tag" || fail "cannot identify release tag from: $cli"
    check_pin corral-next "$cli"
    daemon=$("$bin_txn/new/corrald-next" --version </dev/null) || fail "corrald-next cannot run on this machine; existing install unchanged"
    [ "$daemon" = "corral-daemon ${runtime_tag#v}" ] || fail "daemon version does not match $cli: $daemon"
    pi=$("$bin_txn/new/corral-pi" --version </dev/null) || fail "corral-pi cannot run on this machine; existing install unchanged"
    case "$pi" in
    'corral-pi pi '*) ;;
    *) fail "downloaded Pi host did not identify itself; existing install unchanged" ;;
    esac
}

backup_bins() {
    for name in $names; do
        if [ -e "$bin_dir/$name" ] && [ ! -f "$bin_dir/$name" ]; then
            fail "install destination is not a regular file: $bin_dir/$name"
        fi
        if path_exists "$bin_dir/$name"; then
            cp -pP "$bin_dir/$name" "$bin_txn/old/$name" || fail "cannot back up $bin_dir/$name"
        fi
    done
}

prepare_app() {
    download "$1" "$app_asset"
    # Require a dedicated real directory, which can be exchanged on the same filesystem.
    case "$app_dir" in /*) ;; *) app_dir="./$app_dir" ;; esac
    app_dir=${app_dir%/}
    app_name=${app_dir##*/}
    case "$app_name" in '' | . | ..) fail "unsafe web app directory: $app_dir" ;; esac
    app_parent=${app_dir%/*}
    app_parent=${app_parent:-/}
    mkdir -p "$app_parent"
    app_parent=$(CDPATH='' cd -- "$app_parent" && pwd -P)
    app_dir=${app_parent%/}/$app_name
    if [ "$app_only" = false ]; then
        case "$bin_dir/" in "$app_dir/"*) fail "web app directory would contain the installed programs" ;; esac
        case "$app_dir/" in "$bin_dir/"*) fail "web app directory must be separate from the binary directory" ;; esac
    fi
    web_txn=$(mktemp -d "${app_parent%/}/.corral-app-install.XXXXXX")
    web_site=$web_txn/new/$app_name
    web_backup=$web_txn/old/$app_name
    mkdir "$web_txn/new" "$web_txn/old" "$web_site"
    unpack "$app_asset" "$web_site"
    regular_file "$web_site/index.html" || fail "web app archive has no root index.html"
    if path_exists "$web_site/$web_marker"; then fail "web app archive contains reserved file $web_marker"; fi
    printf '%s\n' "$web_marker_text" >"$web_site/$web_marker"
    # Keep a reference to our staged marker so rollback can identify this particular site.
    ln "$web_site/$web_marker" "$web_txn/staged-marker"
}

check_app_site() {
    if [ -L "$1" ]; then fail "web app directory is a symbolic link: $1"; fi
    [ -d "$1" ] || fail "web app destination is not a directory: $1"
    if [ ! -r "$1" ] || [ ! -x "$1" ]; then fail "cannot inspect web app destination: $1"; fi
    if regular_file "$1/$web_marker" &&
        printf '%s\n' "$web_marker_text" | cmp -s - "$1/$web_marker"; then
        return
    fi
    # Include hidden entries and do not let command substitution strip a filename
    # made entirely of newlines. An inspection error is not proof of emptiness.
    entries=$(LC_ALL=C ls -Aq "$1") || fail "cannot inspect web app destination: $1"
    [ -z "$entries" ] || fail "web app destination is nonempty and not managed by this installer; choose a dedicated directory: $1"
}

install_current() {
    runtime_tag=$version
    if [ "$app_only" = false ]; then
        prepare_runtime
        # If runtime was fetched via latest, select the app from the runtime's exact tag.
        # A release published midway through an install must not mix runtime and app versions.
    fi
    prepare_app "$runtime_tag"
    if [ "$app_only" = false ]; then check_herdr; fi

    # Hold both locks from validation/backup until promotion or rollback is finished.
    # Lock the web parent so equivalent leaf names also contend on case-insensitive filesystems.
    if [ "$app_only" = false ]; then
        lock_install "$bin_dir/.corral-next-install.lock"
        bin_lock=$bin_dir/.corral-next-install.lock
    fi
    lock_install "${app_parent%/}/.corral-web-install.lock"
    web_lock=${app_parent%/}/.corral-web-install.lock
    if same_file "$app_dir" "$web_lock"; then fail "web app destination is reserved: $app_dir"; fi
    if path_exists "$app_dir"; then check_app_site "$app_dir"; fi
    if [ "$app_only" = false ]; then backup_bins; fi

    committing=true
    if [ -d "$app_dir" ]; then
        mv "$app_dir" "$web_txn/old" || fail "cannot back up web app: $app_dir"
        # An independent writer could have changed the site after preflight.
        # Validate the exact backup; on failure cleanup restores or preserves it.
        check_app_site "$web_backup"
    fi
    if path_exists "$app_dir"; then fail "web app destination changed during install: $app_dir"; fi
    # Move to the parent with the exact final basename, not into a destination
    # directory that might have appeared after the preceding check.
    mv "$web_site" "$app_parent" || fail "cannot install web app: $app_dir"
    if [ "$app_only" = false ]; then
        for name in $names; do
            mv -f "$bin_txn/new/$name" "$bin_dir" || fail "cannot install $bin_dir/$name"
        done
    fi
    committing=false
    if [ "$app_only" = false ]; then
        echo "installed $cli, $daemon and $pi to $bin_dir"
        echo "Start the daemon explicitly with: corrald-next --root <workspace> run"
        echo "An already running daemon is not restarted; restart it explicitly to use these binaries."
    fi
    echo "installed the Corral web app ($runtime_tag) to $app_dir"
    echo "Keep this directory dedicated to Corral; upgrades replace its contents."
    echo "Serve this directory with a web server that falls back to index.html for app routes."
}

install_current
if [ "$app_only" = false ]; then
    case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *)
        echo "note: $bin_dir is not on your PATH."
        echo "Add this directory to PATH in your shell profile, then open a new terminal."
        printf "For this shell: export PATH=\"%s:\$PATH\"\n" "$bin_dir"
        ;;
    esac
fi
