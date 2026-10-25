#!/usr/bin/env bash
# Idempotent root setup for the Zubspace production host (Ubuntu 26.04). Copy
# deploy/host/ to the host and run it as root:
#
#   scp -r deploy/host root@zubspace-prod:/root/zubspace-host
#   ssh root@zubspace-prod /root/zubspace-host/setup.sh
#
# Installs rootless Podman for the deploy user, caps every log on the host,
# switches SSH to keys only and enables the firewall.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
user=peter
uid=$(id -u "$user")

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq podman podman-docker docker-compose-v2 ufw jq curl >/dev/null
# Silences podman-docker's "Emulate Docker CLI using podman" notice.
touch /etc/containers/nodocker

# Logs: one journal cap covering every container, hourly rotation of file logs.
install -D -m 644 "$here/journald.conf" /etc/systemd/journald.conf.d/zubspace.conf
systemctl restart systemd-journald
install -D -m 644 "$here/logrotate.conf" /etc/logrotate.d/zubspace
install -d /etc/systemd/system/logrotate.timer.d
printf '[Timer]\nOnCalendar=\nOnCalendar=hourly\n' > /etc/systemd/system/logrotate.timer.d/hourly.conf
systemctl daemon-reload
systemctl restart logrotate.timer

# Rootless Podman: the user's services run without a login session, and
# containers with restart=always come back after a boot.
loginctl enable-linger "$user"
until [ -S "/run/user/$uid/bus" ]; do sleep 1; done
systemctl --user -M "$user@" enable --now podman.socket podman-restart.service
install -d -o "$user" -g "$user" -m 750 "/home/$user/zubspace" "/home/$user/zubspace/prod" "/home/$user/zubspace/dev"

# Key-only SSH, but only once both accounts can log in with a key.
for home in /root "/home/$user"; do
  if [ ! -s "$home/.ssh/authorized_keys" ]; then
    echo "setup: $home/.ssh/authorized_keys is empty; leaving SSH password login on" >&2
    exit 1
  fi
done
install -D -m 644 "$here/sshd.conf" /etc/ssh/sshd_config.d/10-zubspace.conf
sshd -t
systemctl reload ssh

# Firewall: SSH plus each environment's WebRTC UDP range (rtc_udp_port_range in
# deploy/<env>/zubspace_server.toml). The websites arrive through outbound
# Cloudflare tunnels, so no web port is open.
ufw default deny incoming >/dev/null
ufw default allow outgoing >/dev/null
ufw allow 22/tcp comment 'ssh' >/dev/null
ufw allow 9402:9901/udp comment 'zubspace prod webrtc' >/dev/null
ufw allow 9902:10401/udp comment 'zubspace dev webrtc' >/dev/null
ufw --force enable >/dev/null
ufw status verbose
