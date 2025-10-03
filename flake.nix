{
  description = "A basic Flake template";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    systems.url = "github:nix-systems/default";

    flake-parts.url = "github:hercules-ci/flake-parts";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };

    pre-commit-hooks-nix.url = "github:cachix/pre-commit-hooks.nix";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {
    flake-parts,
    systems,
    fenix,
    crane,
    advisory-db,
    pre-commit-hooks-nix,
    treefmt-nix,
    ...
  }:
    flake-parts.lib.mkFlake {inherit inputs;} {
      #see https://flake.parts/options/flake-parts.html

      systems = import systems;

      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.pre-commit-hooks-nix.flakeModule
      ];

      perSystem = {
        config,
        self',
        inputs',
        pkgs,
        system,
        ...
      }: let
        ##################
        ## Rust         ##
        ##################
        rust-stable = inputs'.fenix.packages.stable.completeToolchain;
        rust-nightly = inputs'.fenix.packages.default.toolchain;

        craneLib = (crane.mkLib pkgs).overrideToolchain rust-stable;
        craneLibWithLLVMTools =
          craneLib.overrideToolchain
          (inputs'.fenix.packages.complete.withComponents [
            "cargo"
            "llvm-tools"
            "rustc"
          ]);

        src = craneLib.cleanCargoSource ./.;

        # Common arguments can be set here to avoid repeating them later
        commonArgs = {
          inherit src;

          buildInputs =
            [
              # Add additional build inputs here
              pkgs.openssl
              pkgs.pkg-config
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              # Additional darwin specific inputs can be set here
              pkgs.libiconv
            ];

          # Additional environment variables can be set directly
          # MY_CUSTOM_VAR = "some value";
        };

        # Build *just* the cargo dependencies, so we can reuse
        # all of that work (e.g. via cachix) when running in CI
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the actual crate itself, reusing the dependency
        # artifacts from above.
        mycrate = craneLib.buildPackage (commonArgs
          // {
            inherit cargoArtifacts;
            doCheck = false;
          });

        #Additional Tools:
        custom-tools = [
          (pkgs.writeShellScriptBin
            "cargo-expand"
            ''
              export RUSTC="${rust-nightly}/bin/rustc";
              export CARGO="${rust-nightly}/bin/cargo";
              exec "${pkgs.cargo-expand}/bin/cargo-expand" "$@"
            '')
        ];
      in {
        packages = {
          default = mycrate;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = builtins.attrValues self'.checks;

          nativeBuildInputs =
            [
              rust-stable
              pkgs.bacon
              pkgs.lldb_11
              pkgs.llvm_11

              config.treefmt.build.wrapper
            ]
            ++ custom-tools
            ++ commonArgs.buildInputs
            ++ pkgs.lib.optionals (system == "x86_64-linux") [
              pkgs.cargo-rr
              pkgs.rr
            ];

          shellHook = "
              ${config.pre-commit.installationScript}

              echo Welcome to the devshell of a rust project!
            ";

          RUST_LOG = "info";
        };

        checks =
          {
            # Build the crate as part of `nix flake check` for convenience
            inherit mycrate;

            # Run clippy (and deny all warnings) on the crate source,
            # again, resuing the dependency artifacts from above.
            #
            # Note that this is done as a separate derivation so that
            # we can block the CI if there are issues here, but not
            # prevent downstream consumers from building our crate by itself.
            mycrate-cargo-clippy = craneLib.cargoClippy (commonArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- --deny warnings";
              });

            mycrate-cargo-doc = craneLib.cargoDoc (commonArgs
              // {
                inherit cargoArtifacts;
              });

            # Check formatting
            mycrate-cargo-fmt = craneLib.cargoFmt {
              inherit src;
            };

            # Audit dependencies
            mycrate-cargo-audit = craneLib.cargoAudit {
              inherit src advisory-db;
            };

            # Run tests with cargo-nextest
            # Consider setting `doCheck = false` on `my-crate` if you do not want
            # the tests to run twice
            mycrate-cargo-nextest = craneLib.cargoNextest (commonArgs
              // {
                inherit cargoArtifacts;
                partitions = 1;
                partitionType = "count";
              });
          }
          // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
            # NB: cargo-tarpaulin only supports x86_64 systems
            # Check code coverage (note: this will not upload coverage anywhere)
            mycrate-cargo-coverage = craneLib.cargoTarpaulin (commonArgs
              // {
                inherit cargoArtifacts;
              });
          };

        treefmt = {
          projectRootFile = "./flake.nix";
          # Formatters:
          programs = {
            alejandra.enable = true; # Nix
            black.enable = true; # Python
            rustfmt.enable = true; # Rust
            shellcheck.enable = true; # Bash

            # Use the same rustfmt
            rustfmt.package = inputs'.fenix.packages.stable.rustfmt;
          };
        };
        pre-commit = {
          check.enable = false;
          settings = {
            # Automagically uses the defined treefmt because of https://github.com/cachix/pre-commit-hooks.nix/blob/master/flake-module.nix#L71C13-L71C112
            hooks.treefmt.enable = true;
            hooks.cargo-check.enable = true;
            hooks.commitizen.enable = true;
          };
        };
      };
    };
}
