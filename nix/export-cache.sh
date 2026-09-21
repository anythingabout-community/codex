#!/usr/bin/env bash
# Export only the installed CLI's runtime closure, never the build toolchain.
set -euo pipefail
: "${NIX_CACHE_SIGNING_KEY:?Set the repository secret NIX_CACHE_SIGNING_KEY}"
: "${NIX_SYSTEM:?Set NIX_SYSTEM to the package platform}"
: "${RUNNER_TEMP:?Set RUNNER_TEMP to a temporary directory}"

# macOS commonly exposes temporary directories through /var -> /private/var.
# Nix's local store rejects symlinked parent directories.
runner_temp="$(cd "$RUNNER_TEMP" && pwd -P)"
key_file="$(mktemp "$runner_temp/nix-cache-key.XXXXXX")"
verify_root="$(mktemp -d "$runner_temp/nix-cache-verify.XXXXXX")"
trap 'rm -f "$key_file"; chmod -R u+w "$verify_root"; rm -rf "$verify_root"' EXIT
chmod 600 "$key_file"
printf '%s\n' "$NIX_CACHE_SIGNING_KEY" > "$key_file"
unset NIX_CACHE_SIGNING_KEY

mkdir -p cache
nix key convert-secret-to-public < "$key_file" > cache/cache-public-key.txt
cmp nix/cache-public-key.txt cache/cache-public-key.txt
output="$(readlink -f result)"
nix copy --to "file://$PWD/cache?compression=xz&secret-key=$key_file" "$output"
printf '%s\n' "$output" > "cache/$NIX_SYSTEM.store-path"

# Import into an empty store to check completeness and signatures, even if the
# builder already has every dependency in its usual /nix/store.
nix copy --from "file://$PWD/cache" \
  --to "local?root=$verify_root" \
  --option trusted-public-keys "$(cat cache/cache-public-key.txt)" \
  "$output"
