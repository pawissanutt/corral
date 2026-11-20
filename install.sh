#!/bin/sh
set -eu

repo="${CORRAL_REPO:-pawissanutt/corral}"
version="${CORRAL_VERSION:-latest}"
base="${CORRAL_BASE_URL:-https://github.com/$repo/releases}"
base=${base%/}
bin_dir="${CORRAL_INSTALL_DIR:-$HOME/.local/bin}"
app_dir="${CORRAL_APP_INSTALL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/corral-next/web}"
app_asset=corral-app-web.tar.gz
# Keep in sync with the plugin's min_herdr_version.
min_herdr_version=0.9.3
herdr_installer=https://herdr.dev/install.sh
yes=false
dry_run=false
legacy=${CORRAL_INSTALL_LEGACY:-0}
app_only=false

fail() {
    echo "corral: $*" >&2
    exit 1
}

for arg in "$@"; do
    case "$arg" in
    --yes) yes=true ;;
    --dry-run) dry_run=true ;;
    --legacy) legacy=1 ;;
    --app-only) app_only=true ;;
    *) fail "unknown option: $arg (use --yes, --dry-run, --legacy or --app-only)" ;;
    esac
done
if [ "$legacy" = 1 ] && [ "$app_only" = true ]; then
    fail "--legacy and --app-only are mutually exclusive"
fi

valid_tag() {
    printf '%s\n' "$1" | LC_ALL=C awk '
        NR == 1 && /^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?([+][0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/ { valid=1 }
        END { exit !(NR == 1 && valid) }
    '
}
[ "$version" = latest ] || valid_tag "$version" || fail "invalid release tag: $version (expected latest or vX.Y.Z)"
if [ "$legacy" = 1 ] && [ "$version" = latest ] && [ "$base" = 'https://github.com/pawissanutt/corral/releases' ]; then
    fail "--legacy requires CORRAL_VERSION set to a published legacy tag (vX.Y.Z)"
fi

if [ "$app_only" = false ]; then
    case "$(uname -s)" in
    Linux)
        if [ "$legacy" = 1 ]; then os=unknown-linux-musl; else os=unknown-linux-gnu; fi
        ;;
    Darwin) os=apple-darwin ;;
    *) fail "unsupported OS $(uname -s); on native Windows use install.ps1" ;;
    esac
    case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "unsupported CPU $(uname -m)" ;;
    esac
    if [ "$legacy" = 1 ]; then
        asset="corral-$arch-$os.tar.gz"
    else
        asset="corral-next-$arch-$os.tar.gz"
    fi
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
        if [ "$legacy" = 1 ]; then
            echo "would install corral-pi-host beside corral when the archive has it, then run: corral agent install"
        else
            echo "would install corral-next, corrald-next and corral-pi (no daemon changes)"
        fi
        echo "would check for herdr >= $min_herdr_version, corral's terminal engine"
        echo "if needed, would offer: curl -fsSL $herdr_installer | sh"
    fi
    if [ "$legacy" != 1 ]; then
        app_tag=$version
        if [ "$app_only" = false ] && [ "$version" = latest ]; then app_tag='<runtime tag>'; fi
        echo "would download and verify $(release_url "$app_tag" "$app_asset") into $app_dir (no web server started)"
    fi
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
        if [ "$2" != "$app_asset" ] && [ "$legacy" != 1 ]; then
            fail "download failed: $url (use --legacy with a published legacy tag for the old CLI)"
        fi
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

names='corral-next corrald-next corral-pi'
tmp=$(mktemp -d)
staged=
staged_host=
bin_txn=
web_txn=
committing=false
cleanup() {
    trap '' INT TERM
    if [ "$committing" = true ]; then
        # A missing staged entry was already promoted. Restore the old copy (or absence).
        if [ -n "$bin_txn" ]; then
            for name in $names; do
                if [ ! -e "$bin_txn/$name" ]; then
                    if [ -e "$bin_txn/old/$name" ] || [ -L "$bin_txn/old/$name" ]; then
                        mv -f "$bin_txn/old/$name" "$bin_dir/$name" || {
                            echo "corral: could not restore $bin_dir/$name; backup in $bin_txn/old" >&2
                            keep_bin=true
                        }
                    else
                        rm -f "$bin_dir/$name" || true
                    fi
                fi
            done
        fi
        if [ -n "$web_txn" ]; then
            if [ ! -d "$web_txn/site" ]; then rm -rf "$app_dir" || true; fi
            if [ -d "$web_txn/old" ]; then
                if [ -e "$app_dir" ] || ! mv "$web_txn/old" "$app_dir"; then
                    echo "corral: could not restore $app_dir; backup in $web_txn/old" >&2
                    keep_web=true
                fi
            fi
        fi
    fi
    if [ -n "$staged" ]; then rm -f "$staged"; fi
    if [ -n "$staged_host" ]; then rm -f "$staged_host"; fi
    if [ -n "$bin_txn" ] && [ "${keep_bin:-false}" = false ]; then rm -rf "$bin_txn"; fi
    if [ -n "$web_txn" ] && [ "${keep_web:-false}" = false ]; then rm -rf "$web_txn"; fi
    rm -rf "$tmp"
}
trap cleanup EXIT
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

install_legacy() {
    check_herdr
    download "$version" "$asset"
    mkdir "$tmp/archive"
    unpack "$asset" "$tmp/archive"
    regular_file "$tmp/archive/corral" || fail "archive has no regular corral binary"
    # Releases before 0.4.0 ship corral alone; corral-pi then needs a source install.
    host=false
    if [ -e "$tmp/archive/corral-pi-host" ] || [ -L "$tmp/archive/corral-pi-host" ]; then
        regular_file "$tmp/archive/corral-pi-host" || fail "archive has no regular corral-pi-host binary"
        host=true
    fi
    for name in corral corral-pi-host; do
        [ ! -d "$bin_dir/$name" ] || fail "install destination is a directory: $bin_dir/$name"
    done
    mkdir -p "$bin_dir"
    if [ "$host" = true ]; then
        staged_host=$(mktemp "$bin_dir/.corral-pi-host-install.XXXXXX")
        cp "$tmp/archive/corral-pi-host" "$staged_host"
        chmod 755 "$staged_host"
        host_version=$("$staged_host" --version </dev/null) || fail "downloaded corral-pi-host cannot run on this machine; existing install unchanged"
        case "$host_version" in
        "corral-pi-host "*) ;;
        *) fail "downloaded corral-pi-host did not identify itself; existing install unchanged" ;;
        esac
    fi
    staged=$(mktemp "$bin_dir/.corral-install.XXXXXX")
    cp "$tmp/archive/corral" "$staged"
    chmod 755 "$staged"
    installed_version=$("$staged" --version </dev/null) || fail "downloaded binary cannot run on this machine; existing install unchanged"
    case "$installed_version" in
    "corral "*) ;;
    *) fail "downloaded binary did not identify itself as corral; existing install unchanged" ;;
    esac
    check_pin corral "$installed_version"
    if [ "$host" = true ]; then
        mv -f "$staged_host" "$bin_dir/corral-pi-host"
        staged_host=
    fi
    mv -f "$staged" "$bin_dir/corral"
    staged=
    echo "installed $installed_version to $bin_dir/corral"
    if [ "$host" = true ]; then
        echo "installed $host_version to $bin_dir/corral-pi-host"
        if "$bin_dir/corral" agent install </dev/null >"$tmp/agent-install.log" 2>&1; then
            echo "set up the corral-pi agent host (corral agent install)"
        else
            echo "note: 'corral agent install' failed; corral-pi profiles need it. Output:"
            awk '{ print "  " $0 }' "$tmp/agent-install.log"
        fi
    fi
}

prepare_runtime() {
    download "$version" "$asset"
    mkdir -p "$bin_dir"
    bin_dir=$(CDPATH='' cd -- "$bin_dir" && pwd -P)
    bin_txn=$(mktemp -d "$bin_dir/.corral-next-install.XXXXXX")
    unpack "$asset" "$bin_txn"
    for name in $names; do
        regular_file "$bin_txn/$name" || fail "archive has no regular $name binary"
        chmod 755 "$bin_txn/$name"
        if [ -e "$bin_dir/$name" ] && [ ! -f "$bin_dir/$name" ]; then
            fail "install destination is not a regular file: $bin_dir/$name"
        fi
    done
    cli=$("$bin_txn/corral-next" --version </dev/null) || fail "corral-next cannot run on this machine; existing install unchanged"
    case "$cli" in
    'corral-next '*) runtime_tag=v${cli#corral-next } ;;
    *) fail "downloaded CLI did not identify itself as corral-next; existing install unchanged" ;;
    esac
    valid_tag "$runtime_tag" || fail "cannot identify release tag from: $cli"
    check_pin corral-next "$cli"
    daemon=$("$bin_txn/corrald-next" --version </dev/null) || fail "corrald-next cannot run on this machine; existing install unchanged"
    [ "$daemon" = "corral-daemon ${runtime_tag#v}" ] || fail "daemon version does not match $cli: $daemon"
    pi=$("$bin_txn/corral-pi" --version </dev/null) || fail "corral-pi cannot run on this machine; existing install unchanged"
    case "$pi" in
    'corral-pi pi '*) ;;
    *) fail "downloaded Pi host did not identify itself; existing install unchanged" ;;
    esac
    # Keep previous copies in a separate directory so an archive cannot provide a backup.
    mkdir "$bin_txn/old"
    for name in $names; do
        if [ -e "$bin_dir/$name" ] || [ -L "$bin_dir/$name" ]; then
            cp -pP "$bin_dir/$name" "$bin_txn/old/$name"
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
    if [ -L "$app_dir" ]; then fail "web app directory is a symbolic link: $app_dir"; fi
    if [ -e "$app_dir" ]; then
        [ -d "$app_dir" ] || fail "web app destination is not a directory: $app_dir"
        if ! regular_file "$app_dir/index.html"; then
            for entry in "$app_dir"/* "$app_dir"/.[!.]* "$app_dir"/..?*; do
                if [ -e "$entry" ] || [ -L "$entry" ]; then
                    fail "web app destination must be empty or contain an installed site: $app_dir"
                fi
            done
        fi
    fi
    web_txn=$(mktemp -d "${app_parent%/}/.corral-app-install.XXXXXX")
    mkdir "$web_txn/site"
    unpack "$app_asset" "$web_txn/site"
    regular_file "$web_txn/site/index.html" || fail "web app archive has no root index.html"
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

    committing=true
    if [ -d "$app_dir" ]; then mv "$app_dir" "$web_txn/old" || fail "cannot back up web app: $app_dir"; fi
    mv "$web_txn/site" "$app_dir" || fail "cannot install web app: $app_dir"
    if [ "$app_only" = false ]; then
        for name in $names; do
            mv -f "$bin_txn/$name" "$bin_dir/$name" || fail "cannot install $bin_dir/$name"
        done
    fi
    committing=false
    if [ "$app_only" = false ]; then
        echo "installed $cli, $daemon and $pi to $bin_dir"
        echo "Start the daemon explicitly with: corrald-next --root <workspace> run"
        echo "An already running daemon is not restarted; restart it explicitly to use these binaries."
    fi
    echo "installed the Corral web app ($runtime_tag) to $app_dir"
    echo "Serve this directory with a web server that falls back to index.html for app routes."
}

if [ "$legacy" = 1 ]; then install_legacy; else install_current; fi
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
