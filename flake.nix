{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, fenix, crane, flake-utils }:
    let
      perSystem = flake-utils.lib.eachDefaultSystem (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          toolchain = fenix.packages.${system}.stable.toolchain;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;

          src = let
            htmlFilter = path: _type: builtins.match ".*\\.html$" path != null;
            htmlOrCargo = path: type:
              (htmlFilter path type) || (craneLib.filterCargoSources path type);
          in pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = htmlOrCargo;
          };

          ra-bridge = craneLib.buildPackage {
            inherit src;
            strictDeps = true;
          };
        in
        {
          packages.default = ra-bridge;

          devShells.default = craneLib.devShell {
            packages = with pkgs; [
              rust-analyzer
              wl-clipboard
            ];
            shellHook = ''
              yank() {
                eval "$@"
                local rc=$?
                kitty @ get-text --extent last_cmd_output | wl-copy
                return $rc
              }
            '';
          };
        });
    in
    perSystem // {
      nixosModules.ra-bridge = import ./nix/module.nix self;
    };
}
