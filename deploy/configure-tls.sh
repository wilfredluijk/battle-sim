#!/usr/bin/env bash
set -euo pipefail
/opt/certbot/bin/certbot certonly --non-interactive --agree-tos --register-unsafely-without-email --preferred-profile shortlived --webroot --webroot-path /var/www/acme --ip-address 93.190.187.250 --cert-name battle-sim
install -d /etc/letsencrypt/renewal-hooks/deploy
cat > /etc/letsencrypt/renewal-hooks/deploy/nginx-reload <<'HOOK'
#!/bin/sh
nginx -t && systemctl reload nginx
HOOK
chmod 0755 /etc/letsencrypt/renewal-hooks/deploy/nginx-reload
cat > /etc/systemd/system/battle-cert-renew.service <<'UNIT'
[Unit]
Description=Renew battle-sim IP certificate
After=network-online.target nginx.service
[Service]
Type=oneshot
ExecStart=/opt/certbot/bin/certbot renew --cert-name battle-sim --quiet
UNIT
cat > /etc/systemd/system/battle-cert-renew.timer <<'UNIT'
[Unit]
Description=Check IP certificate renewal twice daily
[Timer]
OnCalendar=*-*-* 00,12:00:00
RandomizedDelaySec=1800
Persistent=true
[Install]
WantedBy=timers.target
UNIT
cat > /usr/local/sbin/battle-cert-check <<'CHECK'
#!/bin/sh
set -eu
openssl x509 -checkend 86400 -noout -in /etc/letsencrypt/live/battle-sim/fullchain.pem
curl --fail --silent --show-error --max-time 5 https://93.190.187.250/bot -o /dev/null || [ "$?" = 22 ]
CHECK
chmod 0755 /usr/local/sbin/battle-cert-check
cat > /etc/systemd/system/battle-cert-check.service <<'UNIT'
[Unit]
Description=Alert in systemd/journal if IP certificate expires within 24 hours
[Service]
Type=oneshot
ExecStart=/usr/local/sbin/battle-cert-check
UNIT
cat > /etc/systemd/system/battle-cert-check.timer <<'UNIT'
[Unit]
Description=Hourly IP certificate expiry monitor
[Timer]
OnCalendar=hourly
Persistent=true
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
systemctl enable --now battle-cert-renew.timer battle-cert-check.timer
