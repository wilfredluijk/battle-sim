#!/usr/bin/env bash
set -euo pipefail
install -m 0755 /opt/battle-sim/retention.py /usr/local/sbin/battle-retention
install -m 0755 /opt/battle-sim/host-monitor.sh /usr/local/sbin/battle-health
cat > /etc/systemd/system/battle-retention.service <<'UNIT'
[Unit]
Description=Retain bounded battle-sim replays
[Service]
Type=oneshot
ExecStart=/usr/local/sbin/battle-retention
UNIT
cat > /etc/systemd/system/battle-retention.timer <<'UNIT'
[Unit]
Description=Daily battle-sim replay retention
[Timer]
OnCalendar=daily
Persistent=true
[Install]
WantedBy=timers.target
UNIT
cat > /etc/systemd/system/battle-health.service <<'UNIT'
[Unit]
Description=Battle-sim application, TLS and free-space monitor
[Service]
Type=oneshot
ExecStart=/usr/local/sbin/battle-health
UNIT
cat > /etc/systemd/system/battle-health.timer <<'UNIT'
[Unit]
Description=Check battle-sim health every five minutes
[Timer]
OnBootSec=5min
OnUnitActiveSec=5min
[Install]
WantedBy=timers.target
UNIT
install -d /etc/systemd/journald.conf.d
cat > /etc/systemd/journald.conf.d/battle-limits.conf <<'UNIT'
[Journal]
SystemMaxUse=200M
MaxRetentionSec=14day
UNIT
if test -f /etc/logrotate.d/nginx; then
    sed -i 's/weekly/daily/; s/rotate 52/rotate 7/; /daily/a\        maxsize 10M' /etc/logrotate.d/nginx
fi
systemctl restart systemd-journald
systemctl daemon-reload
systemctl enable --now battle-retention.timer battle-health.timer
