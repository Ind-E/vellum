{self}: {
  lib,
  pkgs,
  ...
}: let
  tomlFormat = pkgs.formats.toml {};
in {
  options.services.vellum = {
    enable = lib.mkEnableOption "vellum, a live screen annotation overlay for Wayland";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "inputs.vellum.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "Vellum package to use.";
    };
    settings = lib.mkOption {
      type = tomlFormat.type;
      default = {};
      example = {
        default_tool = "arrow";
        remember_last_tool = false;
        default_fill_shapes = true;
        feedback_duration_ms = 250;
        tools.pen.opacity = 0.75;
        palette = [
          "#FF6B6B"
          "#FFD93D"
          "#6BCB77"
          "#4D96FF"
          "#845EC2"
        ];
      };
      description = ''
        Configuration options for vellum.
        NixOS writes system-wide configuration, while Home Manager writes the user's configuration.
        See available options at <https://github.com/greyxp1/vellum#configuration>.
      '';
    };
  };
}
