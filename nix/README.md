# Installing Codex CLI with Nix

This flake builds this repository's Codex CLI on x86_64 and ARM64 Linux/macOS.
It is an installation interface; the development environment uses `devenv.nix`.
Nix must have the `nix-command` and `flakes` experimental features enabled.

## Install or run

After the first successful **Nix CLI and Pages cache** workflow, install the
cached revision shown at <https://anythingabout-community.github.io/codex/>:

```sh
nix profile install --accept-flake-config github:anythingabout-community/codex/fork#codex
codex --version

# Run without adding Codex to your profile.
nix run --accept-flake-config github:anythingabout-community/codex/fork -- --help

# Update an existing installation.
nix profile upgrade --accept-flake-config codex
```

The flake supplies the Pages substituter and its public signing key.
`--accept-flake-config` explicitly trusts that configuration. On a multi-user
installation, the daemon may require an administrator to configure these in
`/etc/nix/nix.conf` (then restart the Nix daemon):

```ini
extra-substituters = https://anythingabout-community.github.io/codex
extra-trusted-public-keys = anythingabout-community-codex-1:iqqNKnjHxIt69VbEDd7f2IcMtMjTe3hUs446pl60PFA=
```

Replace the public-key placeholder with the entire line in
[`cache-public-key.txt`](cache-public-key.txt). Keep the standard
`https://cache.nixos.org` cache enabled. No private key or GitHub login is needed
to download packages.

The Pages landing page identifies the cached commit. To guarantee that an
installation selects that build while a newer branch commit is still building,
replace `fork` in the flake URL with that full commit SHA. A missing cache entry
falls back to a local source build. Older revisions remain buildable from their
locked inputs, but only the most recently deployed revision is retained on Pages.

For NixOS or Home Manager, add this flake as an input and install
`inputs.codex.packages.${pkgs.stdenv.hostPlatform.system}.codex`. Configure the
substituter and public key in `nix.settings` as above. Do not override/follow the
flake's Nixpkgs or Rust inputs if you want the published cache to match.

The package includes `codex`, `codex-code-mode-host`, shell completions, and a
runtime dependency on ripgrep. Optional standalone distribution resources such
as the voice runtime are not bundled.

## GitHub Actions and Pages setup

1. In **Settings → Pages → Build and deployment**, choose **GitHub Actions**.
2. Store a Nix cache private signing key in the repository Actions secret
   `NIX_CACHE_SIGNING_KEY`. Its public key must match `cache-public-key.txt` and
   `flake.nix`. Never commit the private key. To initialize a different cache:

   ```sh
   umask 077
   nix key generate-secret --key-name YOUR-CACHE-NAME-1 > cache-secret.key
   nix key convert-secret-to-public < cache-secret.key > cache-public-key.txt
   gh secret set NIX_CACHE_SIGNING_KEY --repo OWNER/REPO < cache-secret.key
   rm cache-secret.key
   ```

3. For a fork, update the Pages URL and public key in `flake.nix`, the public key
   file, and the installation URLs in this document. Update the workflow's push
   branch filter if the default branch is neither `fork` nor `main`.
4. Push the configuration to the default branch, or run **Nix CLI and Pages
   cache** manually on that branch.

The workflow builds all four platforms natively and runs the package's install
checks. Pull requests build without signing or publishing. Only the default
branch can publish. Each builder exports the full runtime closure, signs it,
and imports it into an empty store to verify completeness and signatures.
The deployment job merges all four caches into one Pages artifact. A failed
build leaves the previous Pages deployment in place.

The site is replaced on each successful deployment, so this workflow owns the
repository's entire Pages site. It enforces a 950 MB content limit to leave room
under GitHub Pages' 1 GB site limit. Build dependencies and intermediate Rust
outputs are not published. GitHub Actions artifacts are temporary transport;
users download from Pages as a normal Nix binary cache.

## Maintaining the package

- `flake.lock` pins Nixpkgs and Rust overlay. Nixpkgs 26.05 retains Intel Mac
  support; check platform support before changing that branch.
- The Rust version comes from `codex-rs/rust-toolchain.toml`.
- When Git dependencies change in `Cargo.lock`, update `cargoLock.outputHashes`
  in `codex-rs/default.nix` using the hashes reported by Nix.
- When V8 changes, update `nix/v8.nix` from the release manifests verified
  against `third_party/v8/rusty_v8_*_release_manifests.sha256`.
- Validate with `nix flake check --all-systems --no-build`, `nix build .#codex`,
  and `actionlint .github/workflows/nix.yml`. The Nix build runs CLI smoke checks;
  it deliberately does not run the full Rust integration suite.
