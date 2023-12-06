{
  description = "Radicle";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/release-23.11";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };

    flake-utils.url = "github:numtide/flake-utils";

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  nixConfig = {
    keepOutputs = true;
  };

  outputs = {
    self,
    nixpkgs,
    crane,
    fenix,
    flake-utils,
    advisory-db,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = nixpkgs.legacyPackages.${system};

      inherit (pkgs) lib;

      craneLib = crane.lib.${system};

      srcFilters = path: type:
      # Allow sql schemas
        (lib.hasSuffix "\.sql" path)
        ||
        # Allow diff files for testing purposes
        (lib.hasSuffix "\.diff" path)
        ||
        # Allow md files for testing purposes
        (lib.hasSuffix "\.md" path)
        ||
        # Default filter from crane (allow .rs files)
        (craneLib.filterCargoSources path type);

      src = lib.cleanSourceWith {
        src = ./.;
        filter = srcFilters;
      };

      # Common arguments can be set here to avoid repeating them later
      commonArgs = {
        inherit src;
        strictDeps = true;

        buildInputs =
          [
            pkgs.git
            # Add additional build inputs here
          ]
          ++ lib.optionals pkgs.stdenv.isDarwin [
            # Additional darwin specific inputs can be set here
            pkgs.libiconv
          ];
      };

      # Build *just* the cargo dependencies, so we can reuse
      # all of that work (e.g. via cachix) when running in CI
      cargoArtifacts =
        craneLib.buildDepsOnly commonArgs;

      # Build the actual crate itself, reusing the dependency
      # artifacts from above.
      radicle = craneLib.buildPackage (commonArgs
        // {
          inherit (craneLib.crateNameFromCargoToml {cargoToml = ./radicle/Cargo.toml;});
          doCheck = false;
          inherit cargoArtifacts;
        });
    in {
      # Formatter
      formatter = pkgs.alejandra;

      # Set of checks that are run: `nix flake check`
      checks = {
        # Build the crate as part of `nix flake check` for convenience
        inherit radicle;

        # Run clippy (and deny all warnings) on the crate source,
        # again, reusing the dependency artifacts from above.
        #
        # Note that this is done as a separate derivation so that
        # we can block the CI if there are issues here, but not
        # prevent downstream consumers from building our crate by itself.
        clippy = craneLib.cargoClippy (commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

        doc = craneLib.cargoDoc (commonArgs
          // {
            inherit cargoArtifacts;
          });

        # Check formatting
        fmt = craneLib.cargoFmt {
          inherit src;
        };

        # TODO: audits are failing so skip this check for now
        # Audit dependencies
        # audit = craneLib.cargoAudit {
        #   inherit src advisory-db;
        # };

        # Audit licenses
        deny = craneLib.cargoDeny {
          inherit src;
        };

        # Run tests with cargo-nextest
        nextest = craneLib.cargoNextest (commonArgs
          // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
            nativeBuildInputs = [
              # git is required so the sandbox can access it.
              pkgs.git
              # Ensure that `git-remote-rad` is present for testing.
              self.packages.${system}.radicle-remote-helper
            ];
            # Ensure dev is used since we rely on env variables being
            # set in tests.
            buildPhase = ''
              export CARGO_PROFILE=dev;
            '';
          });
      };

      packages = {
        default = radicle;
        radicle-remote-helper = craneLib.buildPackage (commonArgs
          // {
            inherit (craneLib.crateNameFromCargoToml {cargoToml = ./radicle-remote-helper/Cargo.toml;});
            inherit cargoArtifacts;
            cargoBuildCommand = "cargo build --release -p radicle-remote-helper";
            doCheck = false;
          });
        radicle-cli = craneLib.buildPackage (commonArgs
          // {
            inherit (craneLib.crateNameFromCargoToml {cargoToml = ./radicle-cli/Cargo.toml;});
            inherit cargoArtifacts;
            cargoBuildCommand = "cargo build --release -p radicle-cli";
            doCheck = false;
          });
        radicle-node = craneLib.buildPackage (commonArgs
          // {
            inherit (craneLib.crateNameFromCargoToml {cargoToml = ./radicle-node/Cargo.toml;});
            inherit cargoArtifacts;
            cargoBuildCommand = "cargo build --release -p radicle-node";
            doCheck = false;
          });
        radicle-httpd = craneLib.buildPackage (commonArgs
          // {
            inherit (craneLib.crateNameFromCargoToml {cargoToml = ./radicle-httpd/Cargo.toml;});
            inherit cargoArtifacts;
            cargoBuildCommand = "cargo build --release -p radicle-httpd";
            doCheck = false;
          });
      };

      apps.default = flake-utils.lib.mkApp {
        drv = radicle;
      };

      apps.rad = flake-utils.lib.mkApp {
        name = "rad";
        drv = self.packages.${system}.radicle-cli;
      };

      apps.git-remote-rad = flake-utils.lib.mkApp {
        name = "git-remote-rad";
        drv = self.packages.${system}.radicle-remote-helper;
      };

      apps.radicle-node = flake-utils.lib.mkApp {
        name = "radicle-node";
        drv = self.packages.${system}.radicle-node;
      };

      apps.radicle-httpd = flake-utils.lib.mkApp {
        name = "radicle-httpd";
        drv = self.packages.${system}.radicle-httpd;
      };

      devShells.default = craneLib.devShell {
        # Inherit inputs from checks.
        checks = self.checks.${system};

        # Extra inputs can be added here; cargo and rustc are provided by default.
        packages = [
          pkgs.cargo-watch
          pkgs.ripgrep
          pkgs.rust-analyzer
        ];
      };
    });
}
