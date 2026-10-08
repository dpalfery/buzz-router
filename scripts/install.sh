#!/bin/sh
# buzz-router installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/dpalfery/buzz-router/main/scripts/install.sh | sh
#
# Downloads the buzz-router binary for this machine from a GitHub Release and
# verifies it against the release's SHA256SUMS.txt before installing. No sudo,
# no compiler, no Rust toolchain. It installs the binary only: it writes no
# config and does not install the service (see docs/guides/install.md).
#
# Options (flags, or the matching env vars):
#   --version <v>      BUZZ_ROUTER_VERSION      release to install, e.g. 0.1.0 or v0.1.0-rc.2
#                                               (default: the latest stable release)
#   --prerelease       BUZZ_ROUTER_PRERELEASE=1 take the newest release, release candidates included
#   --install-dir <d>  BUZZ_ROUTER_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   --help
#
# When piping to sh, pass flags after `-s --`:
#   curl -fsSL <url> | sh -s -- --version 0.1.0-rc.1
#
# To pin the installer itself to a release, fetch it from that release instead
# of from main:
#   curl -fsSL https://github.com/dpalfery/buzz-router/releases/download/v0.1.0/install.sh | sh

set -eu

OWNER="dpalfery"
REPO="buzz-router"
# BUZZ_ROUTER_RELEASE_BASE points at a directory laid out like a GitHub
# release download area (<base>/<tag>/<asset>). It exists so the installer can
# be tested against a local fake release; it must be https:// or file://.
RELEASE_BASE="${BUZZ_ROUTER_RELEASE_BASE:-https://github.com/${OWNER}/${REPO}/releases/download}"
LATEST_URL="https://github.com/${OWNER}/${REPO}/releases/latest"
RELEASES_API="https://api.github.com/repos/${OWNER}/${REPO}/releases?per_page=30"

VERSION="${BUZZ_ROUTER_VERSION:-}"
PRERELEASE="${BUZZ_ROUTER_PRERELEASE:-}"
INSTALL_DIR="${BUZZ_ROUTER_INSTALL_DIR:-}"
WORK=""

log() { printf 'buzz-router: %s\n' "$1" >&2; }

die() {
    printf 'buzz-router: error: %s\n' "$1" >&2
    exit 1
}

usage() {
    cat >&2 <<'EOF'
buzz-router installer for macOS and Linux.

Usage: install.sh [--version <v>] [--prerelease] [--install-dir <dir>]

  --version <v>      release to install, e.g. 0.1.0 or v0.1.0-rc.2 (default: latest stable)
  --prerelease       take the newest release, release candidates included
  --install-dir <d>  where to put the binary (default: ~/.local/bin)

Env vars: BUZZ_ROUTER_VERSION, BUZZ_ROUTER_PRERELEASE=1, BUZZ_ROUTER_INSTALL_DIR.
Through a pipe, pass flags after "-s --":  curl -fsSL <url> | sh -s -- --version 0.1.0
EOF
    exit 0
}

cleanup() {
    if [ -n "$WORK" ] && [ -d "$WORK" ]; then
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT INT TERM

while [ $# -gt 0 ]; do
    case "$1" in
        --version)     [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
        --prerelease)  PRERELEASE=1; shift ;;
        --install-dir) [ $# -ge 2 ] || die "--install-dir needs a value"; INSTALL_DIR="$2"; shift 2 ;;
        -h|--help)     usage ;;
        *)             die "unknown option: $1 (try --help)" ;;
    esac
done

case "$RELEASE_BASE" in
    https://*|file://*) ;;
    *) die "BUZZ_ROUTER_RELEASE_BASE must start with https:// or file://" ;;
esac

[ -n "$INSTALL_DIR" ] || INSTALL_DIR="${HOME:?HOME is not set}/.local/bin"

fetch() {
    # fetch <url> <output-file>
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --proto '=https,file' --tlsv1.2 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        case "$1" in
            file://*) cp "${1#file://}" "$2" ;;
            *) wget -q --https-only -O "$2" "$1" ;;
        esac
    else
        die "need curl or wget"
    fi
}

fetch_stdout() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --proto '=https' --tlsv1.2 "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --https-only -O - "$1"
    else
        die "need curl or wget"
    fi
}

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Darwin)
            # A shell running under Rosetta reports x86_64 on Apple silicon.
            if [ "$arch" = "x86_64" ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then
                arch="arm64"
            fi
            case "$arch" in
                arm64|aarch64) echo "aarch64-apple-darwin" ;;
                x86_64)        echo "x86_64-apple-darwin" ;;
                *) die "unsupported macOS architecture: $arch" ;;
            esac
            ;;
        Linux)
            case "$arch" in
                x86_64|amd64)  echo "x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
                *) die "unsupported Linux architecture: $arch" ;;
            esac
            ;;
        *) die "unsupported OS: $os (Windows uses install.ps1)" ;;
    esac
}

resolve_version() {
    if [ -n "$VERSION" ]; then
        printf '%s\n' "${VERSION#v}"
        return
    fi
    if [ -n "$PRERELEASE" ]; then
        # The releases API lists newest first, drafts excluded for anonymous callers.
        tag="$(fetch_stdout "$RELEASES_API" | sed -n 's/^[[:space:]]*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
    else
        # /releases/latest redirects to /releases/tag/<tag> and skips pre-releases.
        # The redirect costs no API quota.
        if ! command -v curl >/dev/null 2>&1; then
            die "resolving the latest release needs curl; pass --version instead"
        fi
        tag="$(curl -fsSLI --proto '=https' --tlsv1.2 -o /dev/null -w '%{url_effective}' "$LATEST_URL" | sed 's|.*/tag/||')"
    fi
    case "$tag" in
        v[0-9]*) printf '%s\n' "${tag#v}" ;;
        *) die "could not find a release (is there one yet? try --version, or --prerelease)" ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "need sha256sum or shasum to verify the download"
    fi
}

TARGET="$(detect_target)"
VERSION="$(resolve_version)"
TAG="v${VERSION}"
ARCHIVE="buzz-router-${VERSION}-${TARGET}.tar.gz"

WORK="$(mktemp -d)"

log "installing ${VERSION} for ${TARGET}"
fetch "${RELEASE_BASE}/${TAG}/${ARCHIVE}" "${WORK}/${ARCHIVE}" \
    || die "cannot download ${ARCHIVE} from release ${TAG} (does that release exist?)"
fetch "${RELEASE_BASE}/${TAG}/SHA256SUMS.txt" "${WORK}/SHA256SUMS.txt" \
    || die "cannot download SHA256SUMS.txt from release ${TAG}"

# SHA256SUMS.txt lines are "<hash>  <file>". Exact filename match, never a prefix.
expected="$(awk -v f="$ARCHIVE" '$2 == f || $2 == "*" f {print $1; exit}' "${WORK}/SHA256SUMS.txt")"
[ -n "$expected" ] || die "${ARCHIVE} is not listed in SHA256SUMS.txt"
actual="$(sha256_of "${WORK}/${ARCHIVE}")"
if [ "$expected" != "$actual" ]; then
    die "checksum mismatch for ${ARCHIVE} (expected ${expected}, got ${actual}); nothing was installed"
fi
log "checksum ok"

mkdir -p "${WORK}/x"
tar -xzf "${WORK}/${ARCHIVE}" -C "${WORK}/x"
[ -f "${WORK}/x/buzz-router" ] || die "the archive does not contain buzz-router"
chmod 755 "${WORK}/x/buzz-router"

mkdir -p "$INSTALL_DIR"
previous=""
if [ -x "${INSTALL_DIR}/buzz-router" ]; then
    previous="$("${INSTALL_DIR}/buzz-router" --version 2>/dev/null || true)"
fi
# Copy beside the target, then rename: replacing a running binary in place fails
# with "text file busy" on Linux, and a rename is atomic.
cp "${WORK}/x/buzz-router" "${INSTALL_DIR}/.buzz-router.new"
mv -f "${INSTALL_DIR}/.buzz-router.new" "${INSTALL_DIR}/buzz-router"
if [ "$(uname -s)" = "Darwin" ]; then
    xattr -d com.apple.quarantine "${INSTALL_DIR}/buzz-router" 2>/dev/null || true
fi

installed="$("${INSTALL_DIR}/buzz-router" --version 2>&1)" \
    || die "installed to ${INSTALL_DIR}/buzz-router but it does not run: ${installed}"
if [ -n "$previous" ] && [ "$previous" != "$installed" ]; then
    log "upgraded: ${previous} -> ${installed}"
else
    log "installed: ${installed} at ${INSTALL_DIR}/buzz-router"
fi

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        log "${INSTALL_DIR} is not on your PATH. Add it to your shell profile:"
        log "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

cat >&2 <<EOF

Next: write router.toml and roster.toml, store each bot's key, then install the service.
  Getting started: https://github.com/${OWNER}/${REPO}/blob/main/docs/guides/getting-started.md
  Install notes:   https://github.com/${OWNER}/${REPO}/blob/main/docs/guides/install.md
EOF
