# Battle-sim VPS operation

The KaasHosting VPS at **93.190.187.250** runs one room for **eight remote bots**, at **10 Hz**, with a **700×700** map and an **80 ms** command deadline. Physics remains fixed at `DT=0.1`.

## Connect participants

Update/install the Python SDK from this checkout. Each participant receives only their own `.deployment-secrets/playerNN.env` file (01–08). These files contain the public URL and a distinct random credential. The authoritative identities are `player01` through `player08`; a client's requested name cannot impersonate another identity.

```bash
python -m pip install ../battle-sim-python-sdk
# Use your own participant file (no need to source it into your shell):
python ../battle-sim-python-sdk/examples/circle_bot.py --env-file player01.env
```

All six examples accept `--env-file`, inherit `BATTLE_SERVER_URL` and `BATTLE_BOT_TOKEN` when no file is selected, and default to the public training URL. An explicit `--url` or `--host`/`--port` overrides the selected URL. Custom programs may call `run(bot, url="wss://93.190.187.250/bot", token=credential)`. TLS verification is enabled. Do not share the administrator password or the complete roster with participants. See [example connection options and the six-bot Docker setup](../../battle-sim-python-sdk/examples/README.md).

Protocol v3 requires `hello.token`, `ready.config_hash` and `command.match_id`. Update older SDKs/raw clients. The full match configuration arrives before readiness, and any configuration change invalidates readiness. `Bot.accept_configuration(configuration, config_hash)` can refuse settings.

## Open the private administrator UI

From this checkout, run:

```bash
./deploy/admin-tunnel.sh
```

Keep that terminal open and visit **http://127.0.0.1:8787**. Log in using the password in **`.deployment-secrets/admin-password`**. All sensitive REST reads, mutations, spectators and replay access require authentication even over the SSH tunnel. The public HTTPS listener exposes only the exact `/bot` path; the public website root intentionally returns 404.

The scripts use `~/.ssh/id_ed25519`, root SSH and the pinned host key in `deploy/known_hosts`. Set `BATTLE_SSH_KEY` to use a different private key. `-F /dev/null` avoids the broken system SSH proxy configuration found on the build machine.

The admin UI can adjust lobby settings, start a match after all eight bots acknowledge, abort, reset and kick. A kick or disconnect during a match forfeits the ship while retaining its hull, projectile ownership and scoring. Eliminated players stop receiving sensor observations. Equal best HP/ammunition scores draw. Tournament starts use fresh secret seeds and constrained random positions. Monte Carlo is available on a separate local analysis instance; it is intentionally refused by the deployed `--tournament` mode.

## Revoke or rename a participant

The server reads `/opt/battle-sim/secrets/participants.json`. Each entry has `identity`, `token` and `enabled`. Back up and edit that protected file over SSH, then replace it atomically, preserving owner `root:10001` and mode `0440`. The enclosing directory is `0750 root:10001` and is mounted read-only in the container.

Setting `enabled=false` or rotating a token ends its active session within five seconds and blocks new sessions. Tokens must be unique and at least 32 characters; generate them with Python `secrets.token_urlsafe(32)`. Change identities between matches and update the corresponding local participant file. A disconnected identity cannot reconnect during a running match. An operator kick alone is a forfeit; disable the credential to exclude the participant from future matches.

## Deploy a release

Build on the workstation and transfer the image; the VPS does not compile source. Base build images are pinned by digest. The runtime is a static musl executable in a `scratch` image, with a built-in HTTP health probe, numeric UID/GID 10001, a read-only root, all capabilities dropped, no-new-privileges, 1 GiB memory, 1.5 CPU and 128 PID limits. Only `/app/replays` is writable. The application port is published as `127.0.0.1:7878:7878`.

Before a release, run Rust fmt/clippy/tests, Python tests, frontend check/tests/build, and dependency/image scans. Build and export using a fresh release name:

```bash
docker buildx build --platform linux/amd64 --load \
  --build-arg RELEASE=r20260912-3 -t battle-sim:r20260912-3 .
mkdir -p release-artifacts/r20260912-3
docker image save -o release-artifacts/r20260912-3/battle-sim.tar battle-sim:r20260912-3
docker image inspect battle-sim:r20260912-3 --format '{{.Id}}' > release-artifacts/r20260912-3/image-id.txt
cd release-artifacts/r20260912-3
sha256sum battle-sim.tar > battle-sim.tar.sha256
cp ../../compose.prod.yml .
```

Create a release `.env` with `BATTLE_IMAGE=battle-sim:r20260912-3` and `BATTLE_IMAGE_ID=` set to the exact contents of `image-id.txt`. Upload the archive, checksum, image ID, Compose file and `.env` to `/opt/battle-sim/releases/r20260912-3/`. From the repository root:

```bash
./deploy/ssh.sh 'bash -s -- r20260912-3' < deploy/activate-release.sh
```

Activation verifies the archive and loaded image ID, starts Compose with `--no-build --pull never`, waits for health, then updates `/opt/battle-sim/current`. It preserves the previously active release under `/opt/battle-sim/previous`. Retain the current and previous release directories/images; delete older battle-sim releases after checking neither symlink points at them. The initial deployment has no previous production release.

Deploy between matches. A process restart loses the running match and invalidates admin JWTs; replays remain on disk. SIGTERM is handled and either room/network task failing exits the process so Docker can restart it. An interrupted replay remains available for investigation. Each replay has a `.build.json` sidecar containing the exact image ID and release name.

Rollback after a later release:

```bash
./deploy/ssh.sh 'cd /opt/battle-sim/previous && docker compose --env-file .env -f compose.prod.yml up -d --no-build --pull never --wait'
```

Then adjust `current`/`previous` symlinks to reflect the restored release. Never run blanket Docker prune commands on this host.

## Firewall, TLS and monitoring

Nginx serves bot traffic on 443 and only ACME validation on 80. UFW permits 22, 80 and 443 for IPv4/IPv6. The KaasHosting provider firewall **battle-sim-public** is the only firewall attached to this VPS: it permits inbound TCP 22, 80 and 443 from `0.0.0.0/0` and `::/0`, with no other inbound allow rules and unrestricted outbound traffic. The old **Standaard - Alles toestaan** assignment was detached. Preserve this policy when changing provider networking; the application port must stay private. SSH accepts keys only; password and interactive authentication are disabled. Restrict SSH to stable organizer source addresses if available.

Ubuntu maintenance on 2026-09-12 upgraded 154 packages and installed seven kernel dependencies, followed by a successful reboot into `7.0.0-31-generic`. Four updates (`mdadm`, `python3-software-properties`, `software-properties-common`, `sos`) remain deferred by Ubuntu's phased rollout. Schedule later updates between matches and repeat health, TLS and SSH checks after any reboot.

Let’s Encrypt issues the IP certificate at `/etc/letsencrypt/live/battle-sim/`. It is short-lived (approximately six days), so renewal and monitoring are part of normal operation. Certbot runs from `/opt/certbot`.

- `battle-cert-renew.timer`: twice daily, with a randomized delay; reloads Nginx after successful renewal.
- `battle-cert-check.timer`: hourly; fails if the certificate has less than 24 hours remaining or TLS verification fails.
- `battle-health.timer`: every five minutes; checks application readiness, TLS and at least 2 GiB free disk.
- `battle-retention.timer`: daily; keeps at most 200 completed replays, 1 GiB or 30 days, whichever is reached first. Incomplete files older than seven days expire. Files modified within the last hour are protected.
- Container logs: 3×10 MB. Journal: 200 MB / 14 days. Nginx logs rotate daily or at 10 MB.

These checks report through systemd and the journal; no external email/SMS alert destination has been configured. Check them before each training:

```bash
./deploy/ssh.sh 'systemctl --failed; systemctl list-timers "battle-*"; docker ps; df -h /opt/battle-sim'
./deploy/ssh.sh 'journalctl -u battle-cert-renew -u battle-cert-check -u battle-health --since yesterday --no-pager'
./deploy/ssh.sh '/opt/certbot/bin/certbot renew --cert-name battle-sim --dry-run --run-deploy-hooks'
```

Authenticated `GET /api/metrics` exposes accepted/rejected commands, tick count, total/max step time and maximum scheduling delay. The counters reset with the process and include private replay reconstruction work. Use Docker stats and host disk usage alongside them.

## Verify the deployment

With the admin tunnel open, install the SDK and run these **between matches**:

```bash
python deploy/verify-public.py
python deploy/verify-abuse.py
python deploy/verify-restart.py
python deploy/verify-host.py
```

The first script checks private information isolation, participant identity/authentication, duplicate connections, hello timeout, message flood protection and spectator authentication, then runs a complete 3000-tick match with eight real SDK bots over publicly verified TLS and reconstructs the replay. The second checks old-match/future/duplicate commands, live credential revocation and silent readers. The restart test intentionally interrupts a short test match and verifies clean SIGTERM exit, JWT invalidation, lobby recovery and replay persistence. Reports are written under the ignored `release-artifacts/` directory.

Run latency checks from the Amersfoort training venue before fixing the 80 ms rule. Workstation-to-VPS latency is not a venue measurement. For widely dispersed participants, design a separately acknowledged bounded lockstep or delayed-command competition mode; do not increase the deadline beyond the tick interval or change physics by merely raising `--tick-hz`.

Training session history and replay debrief notes are saved in `training-history.json` inside the configured replay directory, so the existing replay volume also persists them. Include that file in replay backups and keep only one live server writer per replay directory. An unfinished round becomes `interrupted` on restart; completed reports, session scoring and saved notes remain available after signing in again. A visible session-storage error means a completed report may still be only in server memory: restore storage or export the history from Sessions before restarting. Session expected-team names do not provision or revoke credentials.
