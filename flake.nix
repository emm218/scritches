{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, fenix, nixpkgs, utils, naersk }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      toolchain = with fenix.packages.${system}; default.toolchain;
      naersk-lib = pkgs.callPackage naersk {
        cargo = toolchain;
        rustc = toolchain;
      };
    in
    {
      formatter.${system} = nixpkgs.legacyPackages.${system}.nixpkgs-fmt;
      packages.${system}.default = with pkgs; naersk-lib.buildPackage {
        src = ./.;
        nativeBuildInputs = [ pkg-config ];
        buildInputs = [ openssl ];
      };
      devShell = with pkgs; mkShell {
        buildInputs = [ toolchain pre-commit rust-analyzer-nightly openssl pkg-config ];
        RUST_SRC_PATH = rustPlatform.rustLibSrc;
      };
    };
}
