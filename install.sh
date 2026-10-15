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
*) fail "unsupported OS $(uname -s); build from source with: cargo install --git https://github.com/$repo" ;;
esac
case "$(uname -m)" in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*) fail "unsupported CPU $(uname -m)" ;;
esac
asset="corral-$arch-$os.tar.gz"
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
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        fail "need curl or wget"
    fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

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
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp" && sha256sum -c "$asset.sha256" >/dev/null) || fail "checksum mismatch for $asset"
elif command -v shasum >/dev/null 2>&1; then
    (cd "$tmp" && shasum -a 256 -c "$asset.sha256" >/dev/null) || fail "checksum mismatch for $asset"
else
    fail "need sha256sum or shasum to verify the download"
fi
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$bin_dir"
cp "$tmp/corral" "$bin_dir/corral.new"
chmod 755 "$bin_dir/corral.new"
mv "$bin_dir/corral.new" "$bin_dir/corral"
echo "installed $("$bin_dir/corral" --version) to $bin_dir/corral"
case ":$PATH:" in
*":$bin_dir:"*) ;;
*) echo "note: $bin_dir is not on your PATH; add it to use 'corral'" ;;
esac
