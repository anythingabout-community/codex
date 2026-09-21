{
  cmake,
  craneLib,
  runCommand,
  llvmPackages,
  openssl,
  libcap,
  rustPlatform,
  pkg-config,
  lib,
  stdenv,
  fetchurl,
  makeWrapper,
  installShellFiles,
  ripgrep,
  version ? "0.0.0",
}:
let
  v8 = import ../nix/v8.nix { inherit fetchurl stdenv; };
  cargoDeps = rustPlatform.importCargoLock {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "appcontainer_common-0.8.0" = "sha256-XUkT2R+RYk9WIqgKnmIAagNW4xOTyp4bWHmQL1iznHw=";
      "crossterm-0.29.0" = "sha256-cQxQQuV+YEutuQiPurXVISq6F/99vCEk8qe5PU8BCSo=";
      "nucleo-0.5.0" = "sha256-Hm4SxtTSBrcWpXrtSqeO0TACbUxq3gizg1zD/6Yw/sI=";
      "runfiles-0.1.0" = "sha256-uJpVLcQh8wWZA3GPv9D8Nt43EOirajfDJ7eq/FB+tek=";
      "tokio-tungstenite-0.28.0" = "sha256-V1xmnrfRWOcZZogelZEA4vvyMj2awCfHVA5/glQ6KAI=";
      "tungstenite-0.27.0" = "sha256-VVHhk7l9J/sEmG3q/UuV/sQ3f+fGsmq5vumSy8vbMvw=";
    };
  };

  # Adapt nixpkgs' pinned Cargo vendor directory to Crane's config layout.
  cargoVendorDir = runCommand "codex-cargo-vendor-config" { } ''
    mkdir -p $out
    substitute ${cargoDeps}/.cargo/config.toml $out/config.toml \
      --replace-fail '"cargo-vendor-dir"' '"${cargoDeps}"'
  '';
  commonArgs = {
    pname = "codex";
    inherit version cargoVendorDir;
    src = lib.cleanSourceWith {
      src = ./.;
      filter = path: type: lib.cleanSourceFilter path type && !(lib.hasSuffix ".nix" path);
    };
    # Build the user-facing CLI and its code execution helper, not the workspace.
    cargoExtraArgs = "--locked --offline -p codex-cli --bin codex -p codex-code-mode-host --bin codex-code-mode-host";
    doCheck = false;

    nativeBuildInputs = [
      cmake
      llvmPackages.clang
      pkg-config
      makeWrapper
      installShellFiles
    ];
    buildInputs = [ openssl ] ++ lib.optionals stdenv.hostPlatform.isLinux [ libcap ];
    env = {
      CC = "${llvmPackages.clang}/bin/clang";
      CXX = "${llvmPackages.clang}/bin/clang++";
      LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
      RUSTY_V8_ARCHIVE = v8.archive;
      RUSTY_V8_SRC_BINDING_PATH = v8.bindings;
      # Release packages do not need the symbolication data used by upstream CI.
      CARGO_PROFILE_RELEASE_DEBUG = "0";
    };

  };
  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs
    // {
      # Installation builds need compiled dependencies, not a separate cargo check.
      buildPhaseCargoCommand = "cargoWithProfile build ${commonArgs.cargoExtraArgs}";
    }
  );
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    passthru = { inherit cargoArtifacts; };

    postInstall = ''
      wrapProgram $out/bin/codex --suffix PATH : ${lib.makeBinPath [ ripgrep ]}
      installShellCompletion --cmd codex \
        --bash <($out/bin/codex completion bash) \
        --fish <($out/bin/codex completion fish) \
        --zsh <($out/bin/codex completion zsh)
    '';

    doInstallCheck = true;
    installCheckPhase = ''
      runHook preInstallCheck
      export HOME="$TMPDIR/codex-home"
      mkdir -p "$HOME"
      $out/bin/codex --version
      $out/bin/codex --help > /dev/null
      test -x $out/bin/codex-code-mode-host
      runHook postInstallCheck
    '';

    meta = {
      description = "OpenAI Codex command-line interface";
      license = lib.licenses.asl20;
      homepage = "https://github.com/anythingabout-community/codex";
      mainProgram = "codex";
      platforms = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
    };
  }
)
