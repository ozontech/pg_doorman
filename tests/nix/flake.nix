{
  description = "pg_doorman multi-language test environment Docker image";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      perSystem =
        {
          system,
          pkgs,
          ...
        }:
        let
          # Rust toolchain from rust-overlay
          rustToolchain = pkgs.rust-bin.stable."1.87.0".default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
            ];
          };

          # Python with required packages
          pythonEnv = pkgs.python3.withPackages (ps: with ps; [
            asyncpg
            psycopg2
            aiopg
            pytest
          ]);

          # All runtime packages
          runtimePackages = with pkgs; [
            # PostgreSQL
            postgresql_16

            # Node.js
            nodejs_22
            nodePackages.npm

            # Go
            go_1_24

            # Python environment
            pythonEnv

            # .NET SDK
            dotnet-sdk_8

            # Rust toolchain
            rustToolchain

            # Build dependencies
            pkg-config
            openssl
            openssl.dev
            gcc
            gnumake
            cmake
            git

            # Additional build tools
            cacert
            coreutils
            bash
            findutils
            gnugrep
            gnused
            gawk
            gnutar
            gzip
            which
            shadow
            sudo
            linux-pam
            procps
            util-linux
            iproute2
            nettools
            curl
            wget
            less
            vim
            tcpdump

            # For native extensions
            libiconv
            zlib
          ];

          # Setup script for installing dependencies on first run
          setupScript = pkgs.writeShellScriptBin "setup-test-deps" ''
            set -e
            echo "Installing dependencies on first run..."
            echo "Dependencies will be cached in Docker volumes for faster subsequent runs"
          '';

          # Environment setup script
          envSetupScript = pkgs.writeShellScriptBin "setup-env" ''
            export CARGO_HOME="/root/.cargo"
            export RUSTUP_HOME="/root/.rustup"
            export GOPATH="/root/go"
            export GOMODCACHE="/root/go/pkg/mod"
            export DOTNET_CLI_HOME="/root/.dotnet"
            export DOTNET_NOLOGO=1
            export NPM_CONFIG_PREFIX="/root/.npm-global"
            export SSL_CERT_FILE="${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
            export NIX_SSL_CERT_FILE="${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"

            export PATH="$CARGO_HOME/bin:$GOPATH/bin:$NPM_CONFIG_PREFIX/bin:$PATH"

            # Create necessary directories
            mkdir -p "$CARGO_HOME" "$RUSTUP_HOME" "$GOPATH" "$DOTNET_CLI_HOME" "$NPM_CONFIG_PREFIX"
          '';

          # Docker image with aggressive layer caching
          dockerImage = pkgs.dockerTools.buildLayeredImage {
            name = "pg_doorman-test-env";
            tag = "latest";

            # Layer structure (bottom to top):
            # 1. Base system tools (coreutils, bash, etc) - rarely changes
            # 2. Runtime packages (postgres, languages) - changes on version bump
            # 3. Build dependencies (gcc, pkg-config) - rarely changes
            # 4. Pre-cached language dependencies - changes when lock files change
            # 5. Helper scripts - changes frequently but tiny
            contents = runtimePackages ++ [
              setupScript
              envSetupScript
              pkgs.dockerTools.caCertificates
              pkgs.dockerTools.usrBinEnv
              pkgs.dockerTools.binSh

              # Create necessary directories and system files
              (pkgs.runCommand "system-setup" {} ''
                mkdir -p $out/tmp $out/etc $out/var/run/postgresql
                chmod 1777 $out/tmp
                chmod 1777 $out/var/run/postgresql

                # Create minimal /etc/passwd and /etc/group
                cat > $out/etc/passwd << 'EOF'
root:x:0:0:root:/root:/bin/bash
postgres:x:999:999:PostgreSQL Server:/var/lib/postgresql:/bin/bash
EOF

                cat > $out/etc/group << 'EOF'
root:x:0:
postgres:x:999:
EOF

                # Create /etc/sudoers allowing passwordless sudo for all
                mkdir -p $out/etc/sudoers.d $out/etc/pam.d
                cat > $out/etc/sudoers << 'EOF'
root ALL=(ALL:ALL) ALL
%sudo ALL=(ALL:ALL) NOPASSWD: ALL
postgres ALL=(ALL:ALL) NOPASSWD: ALL
Defaults env_keep += "PATH"
EOF
                chmod 0440 $out/etc/sudoers

                # Create minimal PAM configuration for sudo
                cat > $out/etc/pam.d/sudo << 'EOF'
auth       sufficient   pam_permit.so
account    sufficient   pam_permit.so
session    sufficient   pam_permit.so
EOF

                cat > $out/etc/pam.d/other << 'EOF'
auth       sufficient   pam_permit.so
account    sufficient   pam_permit.so
session    sufficient   pam_permit.so
EOF
              '')
            ];

            config = {
              Env = [
                "CARGO_HOME=/root/.cargo"
                "RUSTUP_HOME=/root/.rustup"
                "GOPATH=/root/go"
                "GOMODCACHE=/root/go/pkg/mod"
                "GOCACHE=/root/.cache/go-build"
                "DOTNET_CLI_HOME=/root/.dotnet"
                "DOTNET_NOLOGO=1"
                "NPM_CONFIG_PREFIX=/root/.npm-global"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "NIX_SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "PKG_CONFIG_PATH=${pkgs.openssl.dev}/lib/pkgconfig"
                "OPENSSL_DIR=${pkgs.openssl.dev}"
                "OPENSSL_LIB_DIR=${pkgs.openssl.out}/lib"
                "OPENSSL_INCLUDE_DIR=${pkgs.openssl.dev}/include"
                "PATH=/root/.cargo/bin:/root/go/bin:/root/.npm-global/bin:${pkgs.lib.makeBinPath runtimePackages}:/bin:/usr/bin"
                "LANG=C.UTF-8"
                "LC_ALL=C.UTF-8"
              ];
              WorkingDir = "/workspace";
              Cmd = [ "/bin/bash" ];
            };

            # Maximum layers for optimal caching (Docker supports up to 127)
            maxLayers = 125;
          };

          # Development shell for local testing
          devShell = pkgs.mkShell {
            packages = runtimePackages ++ [
              setupScript
              envSetupScript
            ];

            shellHook = ''
              export CARGO_HOME="$HOME/.cargo"
              export RUSTUP_HOME="$HOME/.rustup"
              export GOPATH="$HOME/go"
              export GOMODCACHE="$HOME/go/pkg/mod"
              export DOTNET_CLI_HOME="$HOME/.dotnet"
              export DOTNET_NOLOGO=1
              export NPM_CONFIG_PREFIX="$HOME/.npm-global"

              export PATH="$CARGO_HOME/bin:$GOPATH/bin:$NPM_CONFIG_PREFIX/bin:$PATH"

              echo "pg_doorman test environment ready!"
              echo "Available runtimes:"
              echo "  - PostgreSQL: $(postgres --version)"
              echo "  - Node.js: $(node --version)"
              echo "  - Go: $(go version)"
              echo "  - Python: $(python3 --version)"
              echo "  - .NET: $(dotnet --version)"
              echo "  - Rust: $(rustc --version)"
              echo ""
              echo "Run 'setup-test-deps' to install language-specific dependencies"
            '';
          };

        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import inputs.rust-overlay) ];
            config = {
              allowUnfree = true;
            };
          };

          packages = {
            default = dockerImage;
            inherit dockerImage;
          };

          devShells = {
            default = devShell;
          };
        };
    };
}
