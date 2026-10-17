#!/bin/sh
set -eu

repo="${CORRAL_REPO:-pawissanutt/corral}"
version="${CORRAL_VERSION:-latest}"
base="${CORRAL_BASE_URL:-https://github.com/$repo/releases}"
bin_dir="${CORRAL_INSTALL_DIR:-$HOME/.local/bin}"
# Keep in sync with the plugin's min_herdr_version.
min_herdr_version=0.9.3
herdr_installer=https://herdr.dev/install.sh
yes=false
dry_run=false

fail() {
    echo "corral: $*" >&2
    exit 1
}

for arg in "$@"; do
    case "$arg" in
    --yes) yes=true ;;
    --dry-run) dry_run=true ;;
    *) fail "unknown option: $arg (use --yes or --dry-run)" ;;
    esac
done

case "$(uname -s)" in
Linux) os=unknown-linux-musl ;;
Darwin) os=apple-darwin ;;
*) fail "unsupported OS $(uname -s); on native Windows use install.ps1" ;;
esac
case "$(uname -m)" in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*) fail "unsupported CPU $(uname -m)" ;;
esac
asset="corral-$arch-$os.tar.gz"
case "$version" in
latest) ;;
v[0-9]*)
    case "$version" in
    *[!a-zA-Z0-9.+-]*) fail "invalid release tag: $version (expected vX.Y.Z)" ;;
    esac
    ;;
*) fail "invalid release tag: $version (expected latest or vX.Y.Z)" ;;
esac
if [ "$version" = latest ]; then
    url="$base/latest/download/$asset"
else
    url="$base/download/$version/$asset"
fi

if [ "$dry_run" = true ]; then
    echo "would download $url into $bin_dir"
    echo "would check for herdr >= $min_herdr_version, corral's terminal engine"
    echo "if needed, would offer: curl -fsSL $herdr_installer | sh"
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

tmp=$(mktemp -d)
staged=
cleanup() {
    if [ -n "$staged" ]; then rm -f "$staged"; fi
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

herdr_version=$(herdr --version 2>/dev/null) || herdr_version=
if ! printf '%s\n' "$herdr_version" | herdr_compatible; then
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
        echo "Skipping herdr installation; continuing with corral. Run the command above before 'corral up'."
    fi
fi

fetch "$url" "$tmp/$asset" || fail "download failed: $url"
fetch "$url.sha256" "$tmp/$asset.sha256" || fail "checksum download failed: $url.sha256"
# Accept one checksum for this archive only, not additional filenames or paths.
awk -v asset="$asset" '
    NR == 1 && length($1) == 64 && $1 !~ /[^0-9a-fA-F]/ && $2 == asset && NF == 2 { valid = 1 }
    END { exit !(NR == 1 && valid) }
' "$tmp/$asset.sha256" || fail "invalid checksum manifest for $asset"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp" && sha256sum -c "$asset.sha256" >/dev/null) || fail "checksum mismatch for $asset"
elif command -v shasum >/dev/null 2>&1; then
    (cd "$tmp" && shasum -a 256 -c "$asset.sha256" >/dev/null) || fail "checksum mismatch for $asset"
else
    fail "need sha256sum or shasum to verify the download"
fi
tar -xzf "$tmp/$asset" -C "$tmp" corral || fail "invalid archive: $asset"
if [ ! -f "$tmp/corral" ] || [ -L "$tmp/corral" ]; then
    fail "archive has no regular corral binary"
fi
if [ -d "$bin_dir/corral" ]; then
    fail "install destination is a directory: $bin_dir/corral"
fi
mkdir -p "$bin_dir"
staged=$(mktemp "$bin_dir/.corral-install.XXXXXX")
cp "$tmp/corral" "$staged"
chmod 755 "$staged"
installed_version=$("$staged" --version) || fail "downloaded binary cannot run on this machine; existing install unchanged"
case "$installed_version" in
"corral "*) ;;
*) fail "downloaded binary did not identify itself as corral; existing install unchanged" ;;
esac
if [ "$version" != latest ] && [ "$installed_version" != "corral ${version#v}" ]; then
    fail "version mismatch: requested $version, got $installed_version; existing install unchanged"
fi
mv -f "$staged" "$bin_dir/corral"
staged=
echo "installed $installed_version to $bin_dir/corral"
case ":$PATH:" in
*":$bin_dir:"*) ;;
*)
    echo "note: $bin_dir is not on your PATH."
    echo "Add this directory to PATH in your shell profile, then open a new terminal."
    printf "For this shell: export PATH=\"%s:\$PATH\"\n" "$bin_dir"
    ;;
esac
