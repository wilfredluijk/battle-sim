# Deployment verification — 2026-09-12

Deployed **battle-sim:r20260912-2** on **93.190.187.250**. Container image ID:

`sha256:69f2d5c33e575bb854da2314d41d0a31e4735824b11d4a068d747bedbf54a41c`

The service is healthy and left in an empty lobby after testing. Player endpoint: **wss://93.190.187.250/bot**. Administrator access: [SSH tunnel instructions](README.md).

## Completed

| Review item | Implementation and evidence |
|---|---|
| Participant identity | Eight random credentials, server-reserved identities, one active connection each; wrong/missing tokens and duplicate sessions rejected in public TLS tests. |
| Information isolation | Exact `/bot` proxy; other public routes return 404. Sensitive private reads return 401 without JWT. Spectator requires authentication before any world data. |
| Commands | Exact match/tick, common ingress cutoff, one slot per connection, first valid command wins; old-match, future and duplicate submissions tested live. |
| Limits | HTTP/WS limits, 32 global WebSocket cap, per-IP caps, bounded traffic, write timeouts and heartbeat expiry. Public authenticated flood, idle hello and non-reading client tests passed. |
| Gameplay | Forfeit retains hull/ownership and scoring; eliminated bots stop scouting; tied scores draw; action phases remove fire/powerup ordering advantage. Regression and replay tests passed, including kick/socket cleanup idempotency. |
| Configuration/randomness | Full configuration and SHA-256 acknowledgement; changes invalidate readiness. Fresh secret tournament seeds and constrained random placement; per-bot/tick/purpose derived sensor and powerup streams. Tournament disables offline Monte Carlo mode. |
| Container/release | Static binary in scratch image, UID/GID 10001, read-only root, only replay mount writable, dropped capabilities, no-new-privileges, memory/CPU/PID limits. Local build → archive → SHA-256 check → SCP → image ID check → Compose with no build or pull. |
| Replay persistence | Exclusive filenames and bounded reads/captures retained; v7 simulation compatibility; exact image ID sidecar. Both completed eight-bot replays reconstructed. |
| TLS | Trusted IP certificate; renewal timer, expiry check and proxy reload. Actual Certbot dry-run renewal and deploy hook passed, including a repeated renewal test after the Ubuntu reboot and final provider firewall replacement. |
| Host/network | Key-only SSH, UFW IPv4/IPv6, externally reachable 22/80/443; externally unreachable 7878. Host inspection confirms loopback-only application publish. KaasHosting `battle-sim-public` is the sole attached provider firewall, with inbound TCP 22/80/443 from `0.0.0.0/0` and `::/0`, no other inbound allow rules, and unrestricted outbound traffic. The previous allow-everything assignment was detached through the signed-in Chrome session. |
| Ubuntu maintenance | Upgraded 154 packages and installed seven kernel dependencies with existing configuration preserved. Rebooted successfully into `7.0.0-31-generic`; SSH, Docker and Nginx recovered automatically. Package audit is clean, no reboot remains required, and health/certificate checks and all four timers passed after reboot. Four updates remain deferred by Ubuntu's phased rollout: `mdadm`, `python3-software-properties`, `software-properties-common`, `sos`. |
| Shutdown | SIGTERM exits 0; restart recovered in 2.32 seconds, invalidated old admin JWTs, preserved replays and returned to an empty lobby. Running matches are intentionally not resumed. |
| Retention/monitoring | Container/host log limits, daily replay retention, health/disk/expiry checks and private metrics active. No failed systemd units at verification. |

## Test results

- **212 Rust tests**, fmt and clippy passed.
- **85 Python SDK tests** passed.
- **49 frontend tests**, Svelte/TypeScript check and production build passed after dependency updates.
- Trivy reported **zero known vulnerabilities** in the final image inventory and in a separate scan of Rust/frontend lockfiles including development dependencies. The static image has no OS package inventory; the separate lockfile scan covers application dependencies. Scanner/database evidence is saved locally with the release. This is a scan result, not a guarantee against unknown vulnerabilities.
- Two full eight-bot TLS matches completed. Final release: **300.04 seconds**, 3000 final ticks, **2999 sensor updates received by each bot** (the last tick sends game-over), tied outcome correctly drawn.
- Final sampled WebSocket RTT: **7.8–12.8 ms**, median **8.7 ms**.
- Accepted **23,691** commands; rejected **301** late/wrong-tick commands (**1.25%**). All bots stayed connected. TCP_NODELAY reduced sampled RTT compared with the first run but did **not** eliminate deadline misses.
- Sampled container CPU median **1.62%**, maximum **8.85%**; sampled container memory maximum **1.62 MiB**. These are Docker samples, excluding host/Nginx memory, and include some replay-viewer activity. The soak used stationary active-sensor bots; combat behavior has separate tests.
- Maximum observed simulation step **8.28 ms**; scheduling delay **12.81 ms**. Host had approximately 35 GiB free disk.
- After Ubuntu updates, reboot and provider firewall replacement, a **30.09-second** eight-bot TLS smoke test passed: every bot received **300** ticks, private/public isolation remained intact, and cleanup returned the server to an empty lobby. It recorded **73 late/wrong-tick command errors** across 2400 tick responses (approximately **3.04%**), reinforcing the need for venue timing measurements. Fresh SSH and public IPv4 probes confirmed 22/80/443 reachable and 7878 unreachable. IPv6 rules were verified in the provider panel and UFW; external IPv6 reachability was not measured.

## Remaining organizer actions / limits

1. **Measure from the Amersfoort venue.** The 80 ms deadline includes both network directions and bot computation. The measured 1.25% full-match rejection rate and 3.04% short post-maintenance result mean these workstation tests do not establish that the budget is fair at the venue. Decide the competition timing rule before participants prepare their bots. The server must keep the deadline below its tick interval.
2. **Distribute one participant file per player.** Credentials are in the local ignored `.deployment-secrets/` directory; do not distribute the administrator password or full roster. Monitoring currently reports locally through systemd/journal; no external notification destination was authorized/configured.
3. **Continue normal host maintenance.** Let Ubuntu's four phased updates become eligible and schedule subsequent upgrades between matches. Restrict SSH to stable organizer source addresses where practical.

Raw non-secret scan/test/measurement reports are under `release-artifacts/r20260912-2/` (ignored by Git). Current/previous release symlinks are retained on the VPS. The previous release predates the TCP_NODELAY and duplicate-forfeit cleanup fixes.
