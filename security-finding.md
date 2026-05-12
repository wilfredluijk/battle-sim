# Security findings — bot/server protocol

Scope: `server/src/protocol.rs`, `server/src/net.rs`, `server/src/room.rs`, `server/src/sim/combat.rs`, `server/src/config.rs`, and `docs/PROTOCOL.md`. No rate-limiting code, per-connection size caps, auth, or read timeouts exist today. Analysis is organised by the three questions: protocol/implementation improvements, anti-spoofing, and rate-limit / DDoS surface.

---

## 1. Trust model and spoofing

### What stops impersonation today
- **Server-assigned identity.** `bot_id`/`ship_id` come from `next_index` in `register_bot` (`room.rs:901-904`). A bot cannot pick its own id, and command messages don't carry one — the connection itself is the identity. So a bot can't say "fire as `b_2`": it can only command the ship attached to *its* TCP/WS connection.
- **`/bot` only.** Commands are not accepted from `/spectate` (`net.rs:516` drops anything spectators send).

### What doesn't stop impersonation
- **`hello.name` is completely unvalidated.** Any length, any bytes, no uniqueness check, no charset restriction (`room.rs:888-958`). `PROTOCOL.md` claims "Server may suffix to disambiguate duplicates" but the code doesn't. Two connections claiming the same `name` both register, and the spectator UI just shows the dupes side by side.
- **First-come-first-served slot grab.** An attacker who races their target's bot to `/bot` registers under that name. If the target later connects, they get a different `bot_id`, or — once `max_bots = 4` — get refused with `RoomFull`. With four bots and a known startup time, this is a real attack: connect 4 sockets at `room create`, fill the lobby, kick the real teams out.
- **No authentication, no TLS.** Acknowledged in `CLAUDE.md` as deliberate for local hackathon play. But "local hackathon" usually means everyone is on the same Wi-Fi, where MITM and slot-stealing are trivial.
- **Spectator endpoint leaks ground truth to everyone** (`room.rs:483-558`). A competitor's bot opening a parallel `/spectate` connection sees every ship's `pos`, `heading_deg`, `hp`, and `sensor_mode` every tick — fully bypassing the sensor filter. **This is the single biggest cheating vector in the current design.** The sensor filter at `room.rs:967-996` is doing nothing if the same player can subscribe to `/spectate`.

### Recommended hardening
- **Per-slot join token.** At `room create`, generate one random token per slot, log it to operator stdout. `hello` must include `token`; mismatch → close. Out-of-band token distribution (operator pastes it to each player) makes impersonation require token theft, not just a name guess.
- **Bind `/spectate` separately.** Either:
  - Restrict it to `127.0.0.1` and require an `--operator-port` flag, or
  - Require an operator-issued spectator token, or
  - Add a `--tournament` mode that disables `/spectate` (or sends only what a "neutral observer" should see — e.g. delayed by N ticks).
- **Validate `name`.** `^[A-Za-z0-9_ -]{1,32}$` and reject duplicates outright instead of silently allowing them. Also matters for replay log parsing and the spectator UI.
- **Bind a registered slot to its peer IP** for a grace window. If a bot disconnects mid-match, a reconnect from a different IP should be refused (or require the token again).

---

## 2. Rate limits / DDoS surface

### What's enforced today
- **HTTP head ≤ 8 KiB** (`net.rs:26`). Good.
- **5 protocol violations → close** with `Policy(1008)` (`net.rs:22`). Good for malformed JSON, but does not count "valid frame, sent 10x faster than tick rate."
- **Late command rejected** via wall-clock vs `tick_send_time` (`room.rs:687-732`). Doesn't bound throughput, only timeliness.
- **Sim-side fire rate.** `gun_cooldown_ticks = 15` (1.5 s) and `max_ammo = 20` are enforced inside `combat::fire` (`combat.rs:55-66`). A bot *can* spam `fire` commands every tick, but only one shell every 15 ticks lands; the rest return `cooldown_active` / `no_ammo` errors. So shell spam is bounded by the simulation, **but the error-message spam back to the bot is not**.
- **Outbound to bot uses `try_send`** on a 32-slot mpsc (`room.rs:42`, `room.rs:631`). If a bot is slow / not reading, the room drops frames instead of stalling. Good for server health; means a slow bot silently loses ticks.

### What is unlimited / problematic

| Vector | Where | Impact |
|---|---|---|
| **WebSocket frame size** | `tokio_tungstenite::accept_async` with default config (`net.rs:121`) | tungstenite default is 64 MiB per message. One bot can send a 64 MiB JSON blob; serde_json parses it before rejection. Trivial memory + CPU DoS. |
| **Inbound frames/sec per connection** | `handle_bot` loop (`net.rs:214-336`) | No rate cap. A bot can send 100k `command` frames between ticks. Each parses and pushes to `room_tx` (size 256). |
| **Room event channel is shared** | `ROOM_EVENT_BUFFER = 256` (`room.rs:45`) | One spammy bot fills the queue → other bots' commands block in `room_tx.send().await` → opponents miss their deadline → unfair. Backpressure couples bots that should be isolated. |
| **TCP connections per peer** | `tokio::spawn(handle_connection(...))` (`net.rs:75`) | No cap. Single IP can open thousands of `/bot` and `/spectate` sockets. |
| **Pre-`hello` task lifetime** | `wait_for_hello` (`net.rs:345-431`) | No timeout. A connection can sit forever in this select. Slow Loris on the WebSocket: connect, never send hello, never disconnect. Pre-handshake `read_http_head` (`net.rs:589-611`) has the same problem — it blocks on `stream.read` indefinitely. |
| **NaN / Inf in command floats** | `room.rs:327-329` clamps but doesn't reject | `f32::NAN.clamp(-1.0, 1.0)` returns NaN. NaN propagates into `physics::step_world` → `ship.pos += vel * dt` → NaN position. From then on, every distance calculation against that ship returns NaN, sensor checks misbehave, and replays may differ across architectures. **This is also a determinism risk, not just DoS.** Same goes for `bearing_deg = NaN` in `combat::fire`. |
| **`command.tick` ignored** | `room.rs:687-732` | The protocol says the bot must echo the current tick; the server doesn't check. A bot can send a command labeled `tick: 0` while world is at tick 1500 and it's still applied. Mostly cosmetic, but it means a confused bot is harder to diagnose. |
| **No ping-driven liveness** | (none) | Half-open TCP connections from a dead bot keep the slot held until OS-level keepalive (~2 h). |

### Recommended fixes (cheap → expensive)

1. **Cap WS message size.** `tokio_tungstenite::accept_async_with_config` with `max_message_size: Some(16 * 1024)` and `max_frame_size: Some(16 * 1024)`. JSON commands are <1 KiB — 16 KiB is generous.
2. **Reject non-finite floats** in `BotMsg::Command` validation (between deserialize and queueing). Count as a violation. This closes a determinism hole and a parser CPU sink.
3. **Validate `command.tick`** equals or is at most 1 behind the current room tick — anything else is `invalid_message`.
4. **Coalesce in `net.rs`.** When N `command` frames arrive between two ticks, forward only the most recent to the room. Reduces `room_tx` pressure from O(spam) to O(1). The dropped ones could still count toward the violation budget if they exceed e.g. 4/tick.
5. **Per-connection inbound budget.** Token bucket: e.g. 20 frames/sec sustained, burst 10. Above that, drop and increment violations.
6. **Pre-`hello` and HTTP-head timeouts.** Wrap `read_http_head` and `wait_for_hello` in `tokio::time::timeout(Duration::from_secs(5), ...)`.
7. **Per-IP connection cap.** Maintain `HashMap<IpAddr, u32>` in `net.rs`; refuse new connects above e.g. 8 per IP.
8. **Per-bot inbound channel.** Replace the single `room_tx` (mpsc) with a per-bot inbound mpsc + room fan-in. Spam in one channel can't backlog another.
9. **Server-driven ping every N seconds**, disconnect on missing pong. Frees stuck slots.
10. **Suppress duplicate fire errors.** Currently each rejected `fire` queues an error frame into the bot's outbound buffer (`room.rs:619-639`). At 10 ticks/sec and full spam, that's 10 errors/sec into a 32-slot channel — real `tick` frames get pushed out via `try_send`. Coalesce: at most one fire-error per cooldown cycle.

---

## 3. Minor protocol cleanups (independent of security)

- The `PROTOCOL.md` change-log is empty and the `welcome.version` mentioned in §4 is "planned but absent." Worth wiring before the protocol ossifies.
- The 0° / heading semantics doc-comment in `protocol.rs:11` literally says "is unspecified by the design; treat bearings consistently" — players will trip on this. Pin it. (`combat.rs:145-148` uses `sin / -cos`, i.e. 0° = north, 90° = east. Document this in `PROTOCOL.md`.)
- `Tick.self.rudder/throttle` are echoed back from the last command. Useful, but worth noting in the doc that they reflect the *clamped* server-side value, not whatever the bot last sent.
- Replay logs include the seed but not the protocol/sim version. If you change physics constants mid-tournament, old replays silently desync — record `git_sha` or a `sim_version` in the header.

---

## TL;DR

- **Determinism is well-guarded** (BTreeMap iteration, seeded PCG, BotId order, `f32` discipline). The two leaks are NaN command values and `HashMap`-derived contact-id ordering — worth a one-pass audit.
- **The biggest "competitor cheats" risk isn't spoofing the bot, it's opening `/spectate` to get omniscient ground truth.** Lock that down first.
- **The biggest DoS risk isn't shell spam (sim caps it), it's WS frame size + per-IP connection count + pre-handshake timeouts.** Three small `tokio-tungstenite` config / `tokio::time::timeout` changes close most of it.
- **Slot-stealing is real even without DoS** — a join token per slot is one extra string in `hello` and removes both impersonation and griefing in one go.
