# VPS deployment review

Recorded on 2026-09-12 from the repository review performed on 2026-09-11. Findings describe the working tree inspected during that review; they have not been revalidated against subsequent changes.

**This application should run comfortably on a small VPS, but I would not expose the current deployment directly to the internet.** The container build is already close to what you need; bot authentication, information isolation, and command handling need further work.

The review included local changes arriving during inspection. No implementation changes were made as part of the review.

For **one concurrent game with eight bots running on players’ machines**, I recommend:

| Resource | Recommendation |
|---|---|
| CPU | **2 modern vCPUs**, preferably with consistent CPU allocation |
| RAM | **4 GiB**; 2 GiB is a reasonable minimum to benchmark |
| Disk | **40 GB SSD**, with replay and log retention limits |
| Network | **100 Mbps port**, preferably 1 TB monthly transfer |
| OS | Ubuntu **24.04 or 26.04 LTS**, with Docker Engine and Compose |
| Game settings | `--max-bots 8 --tick-hz 10`, initially retain the `700x700` map |

Both Ubuntu releases are supported by Docker’s installation instructions. [Docker on Ubuntu](https://docs.docker.com/engine/install/ubuntu/).

These are sizing estimates, not measured capacity. The Rust simulation is small, the spectator runs in the browser, and the VPS does not execute the player bots. Kubernetes, a database, and a separate frontend server are unnecessary for this workload.

At an assumed 1–4 KiB per bot update, eight bots at 10 Hz produce roughly **0.7–2.6 Mbps outbound**, before overhead and spectators. Budget **5–10 MB per completed replay** initially and measure actual growth.

The main changes I would require before deployment are:

1. **Authenticate each bot and reserve its identity.**

   The current [`hello` message](server/src/protocol.rs) contains only a name and version. Anyone can join, occupy all eight slots, impersonate a display name, or repeatedly rejoin after a kick.

   Issue a separate, random credential for each participant. Bind it to a server-assigned identity and tournament roster, allow one active connection per identity, and support revocation. Authenticate before allocating a player slot. An IP address is unsuitable as identity because several legitimate players may share a NAT address.

2. **Keep privileged information inaccessible to players.**

   [`net.rs`](server/src/net.rs) exposes spectator, replay, room, and Monte Carlo routes on the same listener. `--tournament` restricts spectator/replay access by peer IP, but other read endpoints remain public. In particular, Monte Carlo status contains its seed, which can reveal deterministic match layouts and randomness.

   Expose only `/bot` publicly. Require administrator authorization for live spectator data, replay access, room internals, and Monte Carlo status. Publish completed replays separately after the match—or after the tournament if they reveal reusable information.

   **Do not rely on the current loopback check behind a proxy.** A proxy can make remote requests appear local; Docker bridge networking can instead make legitimate local requests appear nonlocal. Replace this trust decision with explicit authorization.

3. **Make command acceptance exact and resistant to flooding.**

   In [`room.rs`](server/src/room.rs), commands currently accept a tick range of ±1, and later commands overwrite earlier ones. A repeated command can therefore affect adjacent simulation steps. The deadline uses the time the room processes the command, making queue congestion relevant to acceptance.

   Require a match identifier and the exact tick being answered. Accept one valid command per player per tick, with a documented duplicate policy. Timestamp completed messages at server ingress and apply a common cutoff. Reject old-match, future, and duplicate commands before they enter shared processing.

   The latest local changes observed during the review prioritize timers over events, which addresses the starvation issue initially found. However, flooding can still consume parsing capacity and delay other players’ commands.

4. **Add limits beyond message size.**

   The existing 16 KiB WebSocket limit and protocol-violation handling are useful. Add per-identity message/byte limits, connection-attempt limits, a global connection cap, heartbeat expiry, and write timeouts. Bound each player’s contribution to pending work.

   The advertised handshake timeout currently surrounds the WebSocket `hello` phase; it does not configure the preceding HTTP-header timeout. Likewise, the per-IP cap is applied at WebSocket upgrade, rather than covering every TCP/HTTP connection.

   Enforce HTTP limits at the proxy and message limits inside the application: an HTTP request limiter does not police traffic after WebSocket upgrade. [OWASP WebSocket guidance](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html).

5. **Remove gameplay advantages tied to connections and registration order.**

   The room currently deletes a ship on disconnect, continues generating sensor views for dead ships, and resolves otherwise equal timeout scores using iteration order. These create opportunities for deliberate disconnects, eliminated-player scouting, and joining-order advantages.

   Define a deterministic disconnect/forfeit rule that preserves scoring and world consistency. Stop sensor updates and gameplay commands after elimination. Resolve equal scores as a draw or through an explicitly published rule independent of connection order.

   Also resolve simultaneous actions consistently: processing each bot’s fire and powerup activation sequentially can make same-tick effects depend on bot order.

6. **Control predictable information and freeze match configuration.**

   Normal matches use predictable ring positions and a default seed of `42`. A rotated or shuffled ring still lets a player infer the set of starting positions from its own position and player count.

   Decide whether starting positions are public game information. If they should be hidden, use constrained random placement. Generate fresh secret randomness per match, avoid publishing seeds during play, and avoid treating the shared PCG stream as a security boundary. Independent, reproducible random streams reduce cross-player coupling.

   Before accepting readiness, send every bot the complete final configuration and its version/hash. Configuration can currently change after a bot receives its initial `welcome`; all players must explicitly agree to the same settings.

For the actual VPS, I would use this arrangement:

```text
Players → wss://VPS_IP/bot → Nginx :443 → application container
                                              │
Operator → SSH tunnel → private admin access   └→ replay volume
```

Run Nginx on the host and publish the application container only as **`127.0.0.1:7878:7878`**. Configure the public Nginx listener to proxy the exact `/bot` path and reject other application routes. Access the administrator UI through SSH forwarding, retaining application authentication.

Allow public TCP 443, restrict SSH to operator addresses where practical, and allow TCP 80 only if needed for certificate validation. Apply equivalent IPv6 rules. Use the provider firewall as well as host controls: Docker-published ports can bypass UFW rules. [Docker firewall behavior](https://docs.docker.com/engine/network/packet-filtering-firewalls/).

**Connecting by IP does not require giving up TLS.** Let’s Encrypt now issues publicly trusted IP-address certificates, valid for approximately six days. Automate renewal and proxy reloads, and monitor expiry. Its documented Certbot webroot approach supports IP addresses with Certbot 5.4 or later. [IP certificates](https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability), [Certbot instructions](https://letsencrypt.org/2026/03/11/shorter-certs-certbot).

Two client changes are necessary:

- [`sdk-python/naval_sdk/bot.py`](sdk-python/naval_sdk/bot.py) hardcodes `ws://`. Add a complete server URL, bot authentication, and verified TLS support.
- [`spectator/src/lib/wsClient.ts`](spectator/src/lib/wsClient.ts) also hardcodes `ws://`. Select `wss://` when the page uses HTTPS.

For timing fairness, **80 ms is a tight internet budget**: it includes outbound travel, bot computation, return travel, and server processing. Choose a VPS region near participants and measure their latency before fixing the rules.

For geographically dispersed players, implement either a bounded lockstep mode or a fixed command delay of several ticks. Do not simply increase the deadline beyond the 100 ms tick interval. Also, physics currently uses a fixed `DT = 0.1`; changing `--tick-hz` alone changes wall-clock pacing rather than the simulation timestep.

Remote bot execution cannot enforce equal computing resources or prevent players sharing observations outside the game. If those are strict requirements, submitted bots need to run on organizer-controlled, isolated workers with equal limits and restricted networking. That would require separate capacity planning.

Your preferred **build → SCP → load** workflow is appropriate. The existing [`Dockerfile`](Dockerfile) already embeds the spectator in the server image. Before using it in production:

- Run the application as a non-root UID, with a read-only root filesystem, dropped capabilities, `no-new-privileges`, and bounded memory/process counts.
- Keep only the replay directory writable; rotate container logs and retain a limited number of releases.
- Replace the Compose `change-me` password and remove password logging from [`main.rs`](server/src/main.rs). Keep credentials outside images.
- Pin base images, use `cargo build --release --locked`, align builder/runtime distributions, and scan dependencies and the finished image.
- Handle `SIGTERM`, add health/readiness checks, and make a failed room/network task terminate the process. Currently shutdown waits for Ctrl-C.
- Bound replay reconstruction concurrency and output size. Use collision-resistant replay IDs and exclusive file creation; current second-resolution names plus `File::create` can overwrite previous recordings.

Once a hardened `compose.prod.yml` exists, the release process can be:

```bash
# Build machine: example for an amd64 VPS
docker buildx build --platform linux/amd64 --load \
  -t battle-sim:r1 .

docker image save -o battle-sim-r1.tar battle-sim:r1
sha256sum battle-sim-r1.tar > battle-sim-r1.tar.sha256

# The destination directory must already exist.
scp battle-sim-r1.tar battle-sim-r1.tar.sha256 compose.prod.yml \
  deploy@VPS_IP:/opt/battle-sim/releases/r1/
```

```bash
# VPS, inside the release directory
sha256sum -c battle-sim-r1.tar.sha256
sudo docker image load -i battle-sim-r1.tar
sudo docker compose -f compose.prod.yml up -d --no-build --pull never
```

The production Compose file should reference `battle-sim:r1`, contain no build requirement, and obtain secrets from protected host files. Docker’s image save/load commands preserve image layers and tags. [Save](https://docs.docker.com/reference/cli/docker/image/save/), [Load](https://docs.docker.com/reference/cli/docker/image/load/).

Deploy between matches, retain the previous image and configuration for rollback, and record the exact image ID alongside replays. Restarting this application loses the running match.

Before launch, require an eight-bot soak test through the real TLS proxy, plus tests for duplicate/stale commands, connection churn, slow readers, message floods, hidden-state access, elimination, disconnect scoring, certificate renewal, and restart behavior. Measure tick duration, scheduling delay, command rejection rates, CPU, memory, and disk growth.

The review inspected the implementation and existing replay sizes, but could not run the Rust server or benchmark it: Rust was unavailable on `PATH`, and Docker daemon access was denied. The recommended VPS size therefore needs validation against the hardened release.
