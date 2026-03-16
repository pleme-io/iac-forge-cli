{
  description = "iac-forge-cli — Unified CLI for generating IaC providers from OpenAPI specs";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      substrate,
      ...
    }:
    let
      system = "aarch64-darwin";
      pkgs = import nixpkgs { inherit system; };

      props = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = props.package.version;
      pname = "iac-forge-cli";

      package = pkgs.rustPlatform.buildRustPackage {
        inherit pname version;
        src = pkgs.lib.cleanSource ./.;
        cargoLock = {
          lockFile = ./Cargo.lock;
          outputHashes = { };
        };
        doCheck = true;
        meta = {
          description = props.package.description;
          homepage = props.package.homepage;
          license = pkgs.lib.licenses.mit;
          mainProgram = "iac-forge";
        };
      };

      mkApp = name: script: {
        type = "app";
        program = "${pkgs.writeShellScriptBin name script}/bin/${name}";
      };
    in
    {
      packages.${system} = {
        iac-forge-cli = package;
        default = package;
      };

      overlays.default = final: prev: {
        iac-forge-cli = self.packages.${final.system}.default;
      };

      devShells.${system}.default = pkgs.mkShellNoCC {
        packages = [
          pkgs.rustc
          pkgs.cargo
          pkgs.rust-analyzer
          pkgs.clippy
          pkgs.rustfmt
        ];
      };

      apps.${system} = {
        default = {
          type = "app";
          program = "${package}/bin/iac-forge";
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
          git add Cargo.toml Cargo.lock
          git commit -m "bump: v$NEW"
          git tag "v$NEW"
          echo "bumped: v$CURRENT → v$NEW"
        '';
        release = mkApp "release" ''
          set -euo pipefail
          nix run .#bump -- "''${1:-patch}"
          echo "Release created. Push tags with: git push origin --tags"
        '';
      };

      formatter.${system} = pkgs.nixfmt-tree;
    };
}
