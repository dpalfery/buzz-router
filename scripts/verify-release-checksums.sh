#!/usr/bin/env bash
# Release asset manifest helper.
#
#   scripts/verify-release-checksums.sh --list <version>
#       Print the asset names a release must carry, one per line, SHA256SUMS.txt excluded.
#   scripts/verify-release-checksums.sh --write <dir> <version>
#       Check <dir> holds exactly those assets, then write <dir>/SHA256SUMS.txt over them.
#   scripts/verify-release-checksums.sh --check <dir> <version>
#       Check <dir> holds exactly those assets plus SHA256SUMS.txt, and every hash matches.
#
# Any missing or extra file fails the run, so a build that silently dropped a target
# cannot publish a release.

set -euo pipefail

die() { printf 'verify-release-checksums: error: %s\n' "$1" >&2; exit 1; }

TARGETS=(
    aarch64-apple-darwin
    x86_64-apple-darwin
    x86_64-unknown-linux-gnu
    aarch64-unknown-linux-gnu
    x86_64-pc-windows-msvc
)

expected_assets() {
    version="$1"
    for target in "${TARGETS[@]}"; do
        case "$target" in
            *-windows-*) printf 'buzz-router-%s-%s.zip\n' "$version" "$target" ;;
            *)           printf 'buzz-router-%s-%s.tar.gz\n' "$version" "$target" ;;
        esac
    done
    printf 'install.sh\ninstall.ps1\n'
}

sha256_line() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}

dir_must_match() {
    # dir_must_match <dir> <with-manifest 0|1> <version>
    dir="$1"; with_manifest="$2"; version="$3"
    [ -d "$dir" ] || die "not a directory: $dir"
    want="$(expected_assets "$version")"
    if [ "$with_manifest" = 1 ]; then want="$(printf '%s\nSHA256SUMS.txt\n' "$want")"; fi
    want="$(printf '%s\n' "$want" | sort)"
    have="$(cd "$dir" && find . -maxdepth 1 -type f | sed 's|^\./||' | sort)"
    if [ "$want" != "$have" ]; then
        {
            echo "asset set mismatch in $dir"
            echo "expected:"; printf '%s\n' "$want"
            echo "found:";    printf '%s\n' "$have"
        } >&2
        exit 1
    fi
}

mode="${1:-}"
case "$mode" in
    --list)
        [ $# -eq 2 ] || die "usage: --list <version>"
        expected_assets "$2"
        ;;
    --write)
        [ $# -eq 3 ] || die "usage: --write <dir> <version>"
        dir_must_match "$2" 0 "$3"
        ( cd "$2" && while IFS= read -r f; do sha256_line "$f"; done < <(expected_assets "$3") > SHA256SUMS.txt )
        cat "$2/SHA256SUMS.txt"
        ;;
    --check)
        [ $# -eq 3 ] || die "usage: --check <dir> <version>"
        dir_must_match "$2" 1 "$3"
        ( cd "$2" && while IFS= read -r f; do
            want="$(awk -v f="$f" '$2 == f {print $1}' SHA256SUMS.txt)"
            [ -n "$want" ] || { echo "not in SHA256SUMS.txt: $f" >&2; exit 1; }
            got="$(sha256_line "$f" | awk '{print $1}')"
            [ "$want" = "$got" ] || { echo "hash mismatch: $f" >&2; exit 1; }
        done < <(expected_assets "$3") )
        echo "all $(expected_assets "$3" | wc -l | tr -d ' ') assets match SHA256SUMS.txt"
        ;;
    *) die "usage: $0 --list <version> | --write <dir> <version> | --check <dir> <version>" ;;
esac
