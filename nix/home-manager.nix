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
    home.packages = [cfg.package];
    xdg.configFile."vellum/config.toml" = lib.mkIf (cfg.settings != {}) {
      source = configFile;
    };
    systemd.user.services.vellum = {
      Unit = {
        Description = "Vellum screen annotation overlay";
        After = ["graphical-session.target"];
        PartOf = ["graphical-session.target"];
        X-Restart-Triggers = lib.optional (cfg.settings != {}) configFile;
      };
      Service = {
        Type = "exec";
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
      };
      Install.WantedBy = ["graphical-session.target"];
    };
  };
}
