#!/usr/bin/env bash
# Packages a build for the updater: the asset, a manifest naming its target,
# commit and SHA-256, and the manifest's Ed25519 signature.
#   linux-x86_64   <artifact> is the sonora binary
#   macos-aarch64  <artifact> is the zipped Sonora.app
# Usage: SONORA_UPDATE_SIGNING_KEY="$(cat key.pem)" scripts/sign-release.sh <target> <artifact> <commit> <out-dir>
set -euo pipefail

usage='Usage: scripts/sign-release.sh <target> <artifact> <commit> <out-dir>'
if (( $# != 4 )); then
  printf '%s\n' "$usage" >&2
  exit 1
fi
target=$1
artifact=$2
commit=$3
out=$4
# Asset names must match update::Target::asset.
case "$target" in
  linux-x86_64) asset=sonora-linux-x86_64 mode=755 ;;
  macos-aarch64) asset=sonora-macos-aarch64.zip mode=644 ;;
  *) printf 'Unknown target %s.\n%s\n' "$target" "$usage" >&2; exit 1 ;;
esac
if [[ ! $commit =~ ^[0-9a-f]{40}$ ]]; then
  printf '%s\n' 'The commit must be a full lowercase hash.' >&2
  exit 1
fi
: "${SONORA_UPDATE_SIGNING_KEY:?Set SONORA_UPDATE_SIGNING_KEY to the PEM signing key}"
cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$PWD
name=sonora-$target

install -d "$out"
install -m "$mode" "$artifact" "$out/$asset"
if command -v sha256sum >/dev/null; then
  sha=$(sha256sum "$out/$asset" | cut -d ' ' -f 1)
else
  sha=$(shasum -a 256 "$out/$asset" | cut -d ' ' -f 1)
fi
printf 'sonora-update-v1\ntarget=%s\ncommit=%s\nsha256=%s\n' "$target" "$commit" "$sha" \
  > "$out/$name.manifest"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(umask 077 && printf '%s\n' "$SONORA_UPDATE_SIGNING_KEY" > "$work/key.pem")
openssl pkeyutl -sign -inkey "$work/key.pem" -rawin \
  -in "$out/$name.manifest" -out "$out/$name.manifest.sig"

# Check against the key built into the app, so a wrong secret fails here
# instead of publishing updates no installed copy accepts.
public=$(grep -A 2 '^const RELEASE_KEY' "$root/src/update.rs" | grep -o '[0-9a-f]\{64\}' | head -n 1 || true)
if [[ ! $public =~ ^[0-9a-f]{64}$ ]]; then
  printf '%s\n' 'Could not read RELEASE_KEY from src/update.rs.' >&2
  exit 1
fi
printf '302a300506032b6570032100%s' "$public" | xxd -r -p > "$work/public.der"
openssl pkeyutl -verify -pubin -inkey "$work/public.der" -keyform DER -rawin \
  -in "$out/$name.manifest" -sigfile "$out/$name.manifest.sig" >/dev/null
printf 'Signed %s for %s\n' "$asset" "$commit"
