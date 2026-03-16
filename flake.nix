{
  description = "iac-forge-cli — Unified CLI for generating IaC providers from OpenAPI specs";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crate2nix = {
      url = "github:nix-community/crate2nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    tend = {
      url = "github:pleme-io/tend";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.substrate.follows = "substrate";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      substrate,
      crate2nix,
      tend,
      ...
    }:
    let
      # Dev system (macOS ARM64)
      devSystem = "aarch64-darwin";
      devPkgs = import nixpkgs {
        system = devSystem;
        overlays = [ substrate.rustOverlays.${devSystem}.rust ];
      };

      props = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = props.package.version;
      pname = "iac-forge-cli";

      # Dev package (macOS)
      devPackage = devPkgs.rustPlatform.buildRustPackage {
        inherit pname version;
        src = devPkgs.lib.cleanSource ./.;
        cargoLock = {
          lockFile = ./Cargo.lock;
          outputHashes = { };
        };
        doCheck = true;
        meta = {
          description = props.package.description;
          homepage = props.package.homepage;
          license = devPkgs.lib.licenses.mit;
          mainProgram = "iac-forge";
        };
      };

      mkApp = name: script: {
        type = "app";
        program = "${devPkgs.writeShellScriptBin name script}/bin/${name}";
      };

      # Linux package builder (for Docker images)
      mkLinuxPackage = system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ substrate.rustOverlays.${system}.rust ];
          };
        in
        pkgs.rustPlatform.buildRustPackage {
          inherit pname version;
          src = pkgs.lib.cleanSource ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = { };
          };
          doCheck = false;
          meta = {
            description = props.package.description;
            homepage = props.package.homepage;
            license = pkgs.lib.licenses.mit;
            mainProgram = "iac-forge";
          };
        };

      # Docker image builder
      mkDockerImage = system:
        let
          arch = if system == "aarch64-linux" then "arm64" else "amd64";
          pkgs = import nixpkgs { inherit system; };
          iacForge = mkLinuxPackage system;
          tendPkg = tend.packages.${system}.default;
        in
        pkgs.dockerTools.buildLayeredImage {
          name = "iac-forge";
          tag = "${arch}-latest";
          architecture = arch;
          contents = with pkgs; [
            iacForge
            tendPkg
            git
            cacert
            busybox
          ];
          config = {
            Entrypoint = [ "${tendPkg}/bin/tend" ];
            Env = [
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "RUST_LOG=info"
              "HOME=/home/tend"
              "XDG_CONFIG_HOME=/home/tend/.config"
              "XDG_CACHE_HOME=/home/tend/.cache"
            ];
            WorkingDir = "/workspace";
            User = "1000:1000";
          };
        };

      # Substrate library for image release
      substrateLib = substrate.libFor {
        pkgs = devPkgs;
        system = devSystem;
        forge = null;
      };
    in
    {
      packages.${devSystem} = {
        iac-forge-cli = devPackage;
        default = devPackage;
      };

      # Linux packages for Docker images
      packages.aarch64-linux.default = mkLinuxPackage "aarch64-linux";
      packages.x86_64-linux.default = mkLinuxPackage "x86_64-linux";

      # Docker images
      packages.aarch64-linux.dockerImage = mkDockerImage "aarch64-linux";
      packages.x86_64-linux.dockerImage = mkDockerImage "x86_64-linux";

      overlays.default = final: prev: {
        iac-forge-cli = self.packages.${final.system}.default;
      };

      devShells.${devSystem}.default = devPkgs.mkShell {
        buildInputs = [
          devPkgs.fenixRustToolchain
          devPkgs.rust-analyzer
          devPkgs.cargo-watch
          devPkgs.cargo-edit
          crate2nix.packages.${devSystem}.default
        ];
        RUST_SRC_PATH = "${devPkgs.fenixRustToolchain}/lib/rustlib/src/rust/library";
      };

      apps.${devSystem} = {
        default = {
          type = "app";
          program = "${devPackage}/bin/iac-forge";
        };
        check-all = mkApp "check-all" ''
          set -euo pipefail
          echo "=> cargo fmt --check"
          cargo fmt --check
          echo "=> cargo clippy"
          cargo clippy -- -W clippy::pedantic
          echo "=> cargo test"
          cargo test
          echo "done: all checks passed"
        '';
        test = mkApp "test" ''
          set -euo pipefail
          cargo test
        '';
        bump = mkApp "bump" ''
          set -euo pipefail
          LEVEL=''${1:-patch}
          CURRENT=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
          IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"
          case "$LEVEL" in
            major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
            minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
            patch) PATCH=$((PATCH + 1)) ;;
            *) echo "Usage: bump [major|minor|patch]"; exit 1 ;;
          esac
          NEW="$MAJOR.$MINOR.$PATCH"
          sed -i "" "s/^version = \"$CURRENT\"/version = \"$NEW\"/" Cargo.toml
          cargo check 2>/dev/null || true
          ${crate2nix.packages.${devSystem}.default}/bin/crate2nix generate
          git add Cargo.toml Cargo.lock Cargo.nix
          git commit -m "bump: v$NEW"
          git tag "v$NEW"
          echo "bumped: v$CURRENT → v$NEW"
        '';
        regenerate = mkApp "regenerate" ''
          set -euo pipefail
          echo "Regenerating Cargo.nix..."
          ${crate2nix.packages.${devSystem}.default}/bin/crate2nix generate
          echo "Cargo.nix regenerated."
        '';
        release = mkApp "release" ''
          set -euo pipefail
          nix run .#bump -- "''${1:-patch}"
          echo "Release created. Push tags with: git push origin --tags"
        '';

        # Push Docker images to GHCR
        # Usage: nix run .#push-image
        push-image = mkApp "push-image" ''
          set -euo pipefail
          REGISTRY="ghcr.io/pleme-io/iac-forge"

          echo "=> Building arm64 image..."
          ARM64_IMAGE=$(nix build .#packages.aarch64-linux.dockerImage --no-link --print-out-paths)

          echo "=> Building amd64 image..."
          AMD64_IMAGE=$(nix build .#packages.x86_64-linux.dockerImage --no-link --print-out-paths)

          GIT_SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "dev")

          echo "=> Loading and pushing images..."
          for ARCH_IMAGE in "arm64:$ARM64_IMAGE" "amd64:$AMD64_IMAGE"; do
            ARCH="''${ARCH_IMAGE%%:*}"
            IMAGE="''${ARCH_IMAGE#*:}"
            echo "  Pushing $ARCH image..."
            ${devPkgs.skopeo}/bin/skopeo copy \
              "docker-archive:$IMAGE" \
              "docker://$REGISTRY:$ARCH-$GIT_SHA" \
              --dest-creds "''${GITHUB_ACTOR:-pleme-io}:''${GITHUB_TOKEN}"
            ${devPkgs.skopeo}/bin/skopeo copy \
              "docker-archive:$IMAGE" \
              "docker://$REGISTRY:$ARCH-latest" \
              --dest-creds "''${GITHUB_ACTOR:-pleme-io}:''${GITHUB_TOKEN}"
          done
          echo "=> Done: pushed $REGISTRY:{arm64,amd64}-{$GIT_SHA,latest}"
        '';
      };

      formatter.${devSystem} = devPkgs.nixfmt-tree;
    };
}
