# nix/module.nix — called as: import ./nix/module.nix self
self:
{ config, pkgs, lib, ... }:

let
  cfg = config.services.ra-bridge;
  pkg = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
in {
  options.services.ra-bridge = {
    enable = lib.mkEnableOption "ra-bridge Lutron protocol bridge";
    dataDir = lib.mkOption {
      type = lib.types.path;
      default = "/srv/ra-bridge";
      description = "Directory for ra-bridge config and certificates.";
    };
    telnetPort = lib.mkOption {
      type = lib.types.port;
      default = 6023;
      description = "Port ra-bridge listens on for telnet (NAT'd from 23).";
    };
    webPort = lib.mkOption {
      type = lib.types.port;
      default = 8080;
      description = "Port ra-bridge listens on for HTTP (NAT'd from 80).";
    };
  };

  config = lib.mkIf cfg.enable {
    # Data directories
    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0755 root root -"
      "d ${cfg.dataDir}/certs 0700 root root -"
    ];

    # Main service — serve mode with web UI for setup
    systemd.services.ra-bridge = {
      description = "ra-bridge RadioRA 2 protocol bridge";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" "ra-bridge-nat.service" ];
      wants = [ "ra-bridge-nat.service" ];
      serviceConfig = {
        ExecStart = "${pkg}/bin/ra-bridge serve --config ${cfg.dataDir}/config.toml --certs-dir ${cfg.dataDir}/certs --web-port ${toString cfg.webPort}";
        Restart = "on-failure";
        RestartSec = 5;
        WorkingDirectory = cfg.dataDir;
      };
    };

    # NAT redirect — localhost:23→6023, localhost:80→8080
    # OUTPUT chain handles --network=host Docker containers connecting to 127.0.0.1
    systemd.services.ra-bridge-nat = {
      description = "NAT port redirects for ra-bridge";
      wantedBy = [ "multi-user.target" ];
      before = [ "ra-bridge.service" ];
      after = [ "firewall.service" ];
      partOf = [ "firewall.service" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      path = [ pkgs.iptables ];
      script = ''
        iptables -t nat -A OUTPUT -p tcp -o lo --dport 23 -j REDIRECT --to-port ${toString cfg.telnetPort}
        iptables -t nat -A OUTPUT -p tcp -o lo --dport 80 -j REDIRECT --to-port ${toString cfg.webPort}
        iptables -t nat -A PREROUTING -p tcp --dport 23 -j REDIRECT --to-port ${toString cfg.telnetPort}
        iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port ${toString cfg.webPort}
      '';
      preStop = ''
        iptables -t nat -D OUTPUT -p tcp -o lo --dport 23 -j REDIRECT --to-port ${toString cfg.telnetPort} 2>/dev/null || true
        iptables -t nat -D OUTPUT -p tcp -o lo --dport 80 -j REDIRECT --to-port ${toString cfg.webPort} 2>/dev/null || true
        iptables -t nat -D PREROUTING -p tcp --dport 23 -j REDIRECT --to-port ${toString cfg.telnetPort} 2>/dev/null || true
        iptables -t nat -D PREROUTING -p tcp --dport 80 -j REDIRECT --to-port ${toString cfg.webPort} 2>/dev/null || true
      '';
    };

    # HA starts after ra-bridge (if HA container exists)
    systemd.services.docker-homeassistant = {
      after = [ "ra-bridge.service" ];
      wants = [ "ra-bridge.service" ];
    };

    # Firewall
    networking.firewall.allowedTCPPorts = [ 23 80 cfg.telnetPort cfg.webPort ];
  };
}
