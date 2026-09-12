#!/usr/bin/env bash
set -euo pipefail
if test -e /opt/battle-sim/current; then
    echo "Host already deployed; use activate-release.sh for application releases." >&2
    exit 1
fi
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y ca-certificates curl nginx ufw python3-venv unattended-upgrades
install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
chmod a+r /etc/apt/keyrings/docker.asc
. /etc/os-release
cat > /etc/apt/sources.list.d/docker.sources <<EOF
Types: deb
URIs: https://download.docker.com/linux/ubuntu
Suites: ${UBUNTU_CODENAME:-$VERSION_CODENAME}
Components: stable
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/docker.asc
EOF
apt-get update
apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
install -d -m 0755 /opt/battle-sim/releases /var/www/acme
install -d -m 0700 /opt/battle-sim/secrets
getent group 10001 >/dev/null || groupadd --gid 10001 battle-sim-app
id battle-sim >/dev/null 2>&1 || useradd --system --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin battle-sim
usermod --gid 10001 battle-sim
install -d -o 10001 -g 10001 -m 0750 /opt/battle-sim/replays
python3 -m venv /opt/certbot
/opt/certbot/bin/pip install 'certbot>=5.4,<6'
# Keep key-based root SSH available; disable password and interactive authentication.
cat > /etc/ssh/sshd_config.d/00-battle-sim.conf <<EOF
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
MaxAuthTries 3
LoginGraceTime 20
EOF
sshd -t
systemctl reload ssh
ufw default deny incoming
ufw default allow outgoing
ufw allow 22/tcp comment 'operator SSH key access'
ufw allow 80/tcp comment 'ACME validation only'
ufw allow 443/tcp comment 'authenticated bot websocket'
ufw --force enable
systemctl enable --now docker nginx
# Public HTTP initially serves only ACME challenges.
rm -f /etc/nginx/sites-enabled/default
cat > /etc/nginx/sites-available/battle-sim <<'EOF'
server {
    listen 80;
    listen [::]:80;
    server_name 93.190.187.250;
    location ^~ /.well-known/acme-challenge/ { root /var/www/acme; }
    location / { return 404; }
}
EOF
ln -sfn /etc/nginx/sites-available/battle-sim /etc/nginx/sites-enabled/battle-sim
nginx -t
systemctl reload nginx
/opt/certbot/bin/certbot --version
