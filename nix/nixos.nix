{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.vellum;
  tomlFormat = pkgs.formats.toml {};
  configFile = tomlFormat.generate "vellum-config.toml" cfg.settings;
in {
  imports = [(import ./options.nix {inherit self;})];

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [cfg.package];
    environment.etc."xdg/vellum/config.toml" = lib.mkIf (cfg.settings != {}) {
      source = configFile;
    };
    systemd.user.services.vellum = {
      description = "Vellum screen annotation overlay";
      after = ["graphical-session.target"];
      partOf = ["graphical-session.target"];
      wantedBy = ["graphical-session.target"];
      restartTriggers = lib.optional (cfg.settings != {}) configFile;
      serviceConfig = {
        Type = "exec";
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
      };
    };
  };
}
