{ config, pkgs, lib, ... }:

{
  languages.rust = {
    enable = true;
    toolchainFile = ./codex-rs/rust-toolchain.toml;
    # Use the matching standard-library sources for editor integration.
    toolchain.rust-src = config.languages.rust.toolchainPackage;
    # rust-analyzer is not one of the components in the toolchain file.
    lsp.package = pkgs.rust-analyzer;
  };

  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
    # Corepack uses the pnpm version pinned in package.json.
    corepack.enable = true;
  };

  packages = with pkgs; [
    bazelisk
    cargo-insta
    cargo-nextest
    cmake
    dotslash
    git
    glib
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    libopus
    just
    llvmPackages.clang
    llvmPackages.libclang.lib
    openssl
    pkg-config
    python3
    ripgrep
    uv
  ] ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
    pkgs.alsa-lib
    pkgs.bubblewrap
    pkgs.libcap
  ];

  # Bazelisk reads .bazelversion; repository recipes invoke it as bazel.
  scripts.bazel.exec = ''
    exec ${pkgs.bazelisk}/bin/bazelisk "$@"
  '';

  env = {
    # BoringSSL requires Clang to avoid GCC warnings treated as errors.
    CC = "${pkgs.llvmPackages.clang}/bin/clang";
    CXX = "${pkgs.llvmPackages.clang}/bin/clang++";
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    PKG_CONFIG_PATH = lib.makeSearchPathOutput "dev" "lib/pkgconfig" (
      [
        pkgs.openssl
        pkgs.glib
        pkgs.gst_all_1.gstreamer
        pkgs.gst_all_1.gst-plugins-base
        pkgs.libopus
      ] ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
        pkgs.alsa-lib
        pkgs.libcap
      ]
    );
  } // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
    # Sandboxed subprocesses must resolve standard libraries from system paths.
    # The Nix compiler wrapper otherwise adds libiconv and libintl from the store.
    CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER = "/usr/bin/clang";
    CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER = "/usr/bin/clang";
    # Avoid hundreds of thousands of loose objects slowing CFBundle discovery.
    CARGO_PROFILE_DEV_SPLIT_DEBUGINFO = "packed";
  };
}
