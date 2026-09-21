{ fetchurl, stdenv }:
let
  # Match the sandbox-enabled artifacts used by .github/actions/setup-rusty-v8.
  # Hashes come from the manifests pinned in third_party/v8.
  version = "150.4.0";
  target = stdenv.hostPlatform.rust.rustcTarget;
  hashes = {
    aarch64-apple-darwin = {
      archive = "00adbb48798848c77550441c68673a5e8529b8e1b73eabcdee232cb39b40f4a1";
      bindings = "ca5adf0cf89c9a70ad460ae73648b2fe89b74aa113b3cb7f757b6a02b758394f";
    };
    x86_64-apple-darwin = {
      archive = "e0d9bb64e8b3a034c2930c83972f3f35760211148342fa0407b38250ef330856";
      bindings = "ca5adf0cf89c9a70ad460ae73648b2fe89b74aa113b3cb7f757b6a02b758394f";
    };
    aarch64-unknown-linux-gnu = {
      archive = "d1517eed405468537029b005d5fe997ec74d5c8d351f916b3a6df20b7d2811ba";
      bindings = "7727826ae479bdb645e807239fb12d1f8e2e23de7a6cf16f5ee592690d1d8506";
    };
    x86_64-unknown-linux-gnu = {
      archive = "a35c75d1f26e6a983885a45b33490a4ebe54f05050568b32b89cfb421b30b583";
      bindings = "7727826ae479bdb645e807239fb12d1f8e2e23de7a6cf16f5ee592690d1d8506";
    };
  };
  baseUrl = "https://github.com/openai/codex/releases/download/rusty-v8-v${version}";
in
assert
  (builtins.fromTOML (builtins.readFile ../codex-rs/Cargo.toml)).workspace.dependencies.v8
  == "=${version}";
{
  archive = fetchurl {
    url = "${baseUrl}/librusty_v8_ptrcomp_sandbox_release_${target}.a.gz";
    sha256 = hashes.${target}.archive;
  };
  bindings = fetchurl {
    url = "${baseUrl}/src_binding_ptrcomp_sandbox_release_${target}.rs";
    sha256 = hashes.${target}.bindings;
  };
}
