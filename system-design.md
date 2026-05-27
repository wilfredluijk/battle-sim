# Naval Battle Programming Game — System Design

> A hackathon-friendly programming game where players write algorithms to control a battleship and compete against other players' algorithms in a deterministic top-down naval simulation.

---

## 1. Goals & Constraints

**Goals**
- Players write bots in any language and connect them to a central game server.
- The server runs a deterministic, authoritative simulation.
- Spectators watch a shared top-down visualization in the browser.
- The game is simple enough to ship in a hackathon timebox, deep enough to reward tactical thinking.

**Constraints (scoped for hackathon use)**
- **Local play only.** Server, bots, and spectator all run on the same LAN. No auth, no TLS, no cloud deployment.
- **Bots connect remotely** to the server over the network — the server never executes bot code.
- **Server is implemented in Rust.**
- **Deterministic simulation** — given the same seed and the same sequence of commands, the result is bit-for-bit identical. Replays are free.
- **CLI-driven room lifecycle** — the server operator manages rooms (create, start, end, list) via stdin commands or CLI args.

**Non-goals for MVP**
- Matchmaking, accounts, persistence, leaderboards.
- Sandboxed bot hosting.
- Anti-cheat beyond basic command validation.
- Mobile clients, fancy graphics.

---

## 2. High-Level Architecture

```
┌──────────────┐         ┌─────────────────────────┐         ┌──────────────────┐
│  Bot (Py)    │◄───────►│                         │◄───────►│  Spectator       │
└──────────────┘   WS    │                         │   WS    │  (browser/canvas)│
                         │   Game Server (Rust)    │         └──────────────────┘
┌──────────────┐         │                         │
│  Bot (JS)    │◄───────►│   - Tick scheduler      │
└──────────────┘   WS    │   - Deterministic sim   │
                         │   - Room manager        │
┌──────────────┐         │   - WS fan-out          │
│  Bot (Go)    │◄───────►│   - CLI control plane   │
└──────────────┘   WS    └────────────┬────────────┘
                                      │
                                      ▼
                              ┌───────────────┐
                              │ Operator CLI  │
                              │ (stdin cmds)  │
                              └───────────────┘
```

Three components, three responsibilities:

| Component | Responsibility | Trust |
|---|---|---|
| **Game Server** | Authoritative simulation, fan-out, room lifecycle | Trusted |
| **Bot** | Receives filtered world view, returns commands | Untrusted |
| **Spectator** | Read-only viewer of full world state | Untrusted (read-only) |

The server is the only piece that holds ground truth. Bots receive *filtered* views (sensor-limited). Spectators receive the *full* state for rendering.

---

## 3. Server Design (Rust)

### 3.1 Process Layout

A single Rust binary with three concurrent concerns, coordinated via channels:

- **Network task** — `tokio` + `tokio-tungstenite` accepting WebSocket connections on two paths: `/bot` and `/spectate`. Each connection becomes a lightweight task that pipes messages into the room's mailbox.
- **Simulation task** — owns the world state for a room. Wakes on a fixed-rate timer (the tick), drains pending bot commands from its mailbox, advances physics, and broadcasts state.
- **Control task** — reads stdin line-by-line and dispatches operator commands to the room registry.

Recommended crates:

- `tokio` — async runtime
- `tokio-tungstenite` — WebSocket
- `serde` + `serde_json` — protocol serialization
- `glam` — vector math (use `f32` consistently for determinism, see §6)
- `rand` + `rand_pcg` — seeded deterministic RNG
- `clap` — CLI argument parsing
- `tracing` — structured logging

### 3.2 Room Lifecycle

A **room** is a single match. The server can host multiple rooms simultaneously, but for a hackathon MVP one room at a time is fine.

States:

```
   ┌──────┐  create   ┌────────┐  all bots ready  ┌─────────┐  last ship  ┌────────┐
   │ none │──────────►│ lobby  │─────────────────►│ running │────────────►│ ended  │
   └──────┘           └────────┘                  └─────────┘             └────────┘
                          │                            │                       │
                          │  cancel                    │  abort                │  reset
                          └────────────────────────────┴───────────────────────┘
```

- **Lobby**: bots can connect, register, optionally `select_powerups`, and signal `ready`. Operator can start when ready.
- **Running**: tick loop active. New bot connections rejected. Spectators can join any time.
- **Ended**: final state frozen and broadcast. Replay log written to disk.

### 3.3 Operator CLI

The server accepts CLI arguments for one-shot configuration and stdin commands for live control.

**Launch flags**

```
naval-server \
  --port 7878 \
  --tick-hz 10 \
  --tick-deadline-ms 80 \
  --map 700x700 \
  --max-bots 24 \
  --seed 42 \
  --replay-dir ./replays
```

**Stdin commands** (one per line, while server is running)

| Command | Effect |
|---|---|
| `room create <name>` | Create a new lobby |
| `room list` | Print all rooms with state and player counts |
| `room start <name>` | Transition room from lobby to running |
| `room abort <name>` | End a running room immediately |
| `room kick <name> <bot_id>` | Disconnect a bot |
| `seed <value>` | Set RNG seed for the next room created |
| `quit` | Graceful shutdown |

For the hackathon MVP, the simplest viable shape is: launch the server with flags, it auto-creates one room called `main`, operator types `start` when everyone has connected and signaled ready.

### 3.4 Tick Loop (Pseudocode)

```rust
loop {
    let tick_start = Instant::now();
    let deadline = tick_start + Duration::from_millis(tick_deadline_ms);

    // 1. Send each bot its filtered sensor view for this tick
    for bot in &room.bots {
        let view = compute_sensor_view(&world, bot);
        bot.send(ServerMsg::Tick { tick: room.tick, view });
    }

    // 2. Collect commands until deadline (or all bots responded)
    let commands = collect_commands_until(&room, deadline);

    // 3. Apply commands deterministically (sorted by bot_id for stable order).
    //    Each command may carry an `activate_powerup` resolved alongside fire.
    apply_commands(&mut world, commands);

    // 4. Advance physics one tick (effective speed/accel/turn read powerup state)
    step_physics(&mut world, dt);

    // 5. Resolve weapons, damage, deaths (per-shell heavy buff baked at fire time;
    //    reinforced_hull scales incoming damage; counter_battery_trace arms here)
    resolve_combat(&mut world);

    // 5b. Powerup maintenance: repair_drones regen, GC expired smoke clouds & decoys.
    step_powerup_maintenance(&mut world);

    // 6. Broadcast full state to spectators
    broadcast_spectator(&world);

    // 7. Append tick record to replay log
    replay.append(&world, &commands);

    // 8. Check end condition
    if alive_count(&world) <= 1 {
        end_room(&room);
        break;
    }

    room.tick += 1;
    sleep_until(tick_start + tick_period);
}
```

Critical: commands are applied in a **stable order** (sorted by bot ID), not in arrival order. This is essential for determinism — see §6.

---

## 4. Wire Protocol

**Transport**: WebSocket over TCP. **Encoding**: JSON (text frames). One JSON object per frame, no batching.

JSON is the right call for a hackathon: human-readable when debugging, every language has it, no codegen step. Performance is a non-issue at 10 Hz with a few players.

### 4.1 Bot Endpoint: `ws://<server>:<port>/bot`

**Bot → Server**

```jsonc
// 1. Initial handshake (sent immediately after connect)
{ "type": "hello", "name": "captain_kirk", "version": "1.0" }

// 2. Signal ready (after hello acknowledged)
{ "type": "ready" }

// 3. Each tick, in response to a `tick` message
{
  "type": "command",
  "tick": 142,                  // must echo the tick from the server
  "throttle": 0.8,              // -1.0 (full reverse) to 1.0 (full ahead)
  "rudder": -0.3,               // -1.0 (hard port) to 1.0 (hard starboard)
  "fire": {                     // optional; omit if not firing this tick
    "bearing_deg": 47.5,        // absolute compass bearing
    "range": 300.0              // requested travel distance for the shell
  },
  "sensor_mode": "active"       // "active" | "passive"
}
```

**Server → Bot**

```jsonc
// 1. Hello acknowledged + assignment
{
  "type": "welcome",
  "bot_id": "b_3",
  "ship_id": "s_3",
  "map": { "width": 1000, "height": 1000 },
  "tick_hz": 10,
  "ship_specs": { /* see §5.2 */ }
}

// 2. Game start
{ "type": "game_start", "tick": 0, "starting_position": [120.0, 340.0], "starting_heading_deg": 90.0 }

// 3. Each tick: filtered sensor view
{
  "type": "tick",
  "tick": 142,
  "deadline_ms": 80,
  "self": {
    "pos": [203.4, 511.7],
    "heading_deg": 92.3,
    "speed": 4.1,
    "hp": 78,
    "ammo": 14,
    "rudder": -0.3,
    "throttle": 0.8
  },
  "contacts": [                 // only what this bot's sensors detect
    {
      "id": "c_a1",             // unstable per-tick contact ID, NOT the ship_id
      "kind": "ship",           // "ship" | "shell" | "unknown"
      "pos": [450.0, 510.0],    // possibly noisy
      "bearing_deg": 88.0,      // always present
      "range": 247.0,           // present if active radar
      "confidence": 0.85
    }
  ],
  "events": [                   // events since last tick that this bot can perceive
    { "type": "hit", "amount": 12 },
    { "type": "shell_splash", "pos": [220.0, 505.0] }
  ]
}

// 4. Game end
{
  "type": "game_over",
  "winner": "b_3",              // or null for draw
  "final_tick": 1843,
  "replay_id": "match_20260508_171203"
}

// 5. Errors (invalid command, late command, malformed JSON, etc.)
{ "type": "error", "code": "late_command", "message": "command for tick 142 arrived after deadline" }
```

**Late or missing commands**: the bot's previous throttle/rudder/sensor_mode persist; no shot is fired that tick. The bot is *not* disconnected for missing a tick — only for malformed messages or repeated protocol violations.

### 4.2 Spectator Endpoint: `ws://<server>:<port>/spectate`

Spectators receive **full ground-truth state** every tick, plus all events. No commands accepted.

```jsonc
{
  "type": "world",
  "tick": 142,
  "ships": [
    { "id": "s_1", "bot_name": "captain_kirk", "pos": [203.4, 511.7], "heading_deg": 92.3, "hp": 78, "alive": true },
    /* ... */
  ],
  "shells": [
    { "id": "sh_22", "pos": [310.0, 500.0], "vel": [40.0, 5.0], "ttl_ticks": 18 }
  ],
  "events": [ /* hits, splashes, deaths */ ]
}
```

---

## 5. Gameplay Mechanics (MVP)

The MVP picks the **smallest set of mechanics that produce real tactical depth**. Every feature here exists because removing it makes the game shallow.

### 5.1 The World

- **2D plane**, configurable via `--map WIDTHxHEIGHT` (default 700 × 700 units; you can think of units as meters). No terrain, no obstacles in MVP.
- **Wrapping**: hard walls. Ships hitting walls take a small bump damage and stop.
- **Coordinates**: origin top-left, +x right, +y down (matches canvas conventions for the spectator).
- **Tick rate**: 10 Hz. `dt = 0.1s`.

### 5.2 The Ship

Every ship is identical (no asymmetric balance to worry about for MVP).

| Property | Value |
|---|---|
| Max forward speed | 9.0 units/s |
| Max reverse speed | 2.0 units/s |
| Acceleration | 3.5 units/s² |
| Turn rate at full rudder | 20°/s (scales linearly with speed; stationary ships barely turn) |
| Hull HP | 100 |
| Ammo | 250 shells |
| Gun cooldown | 1.5s (15 ticks) |
| Hit radius (collision/hit) | 8 units |

> **Source of truth.** These numbers live in `server/src/sim/constants.rs` and are surfaced verbatim in the `welcome` payload's `ship_specs` field. If you change one, change the constant — the table tracks the code, not the other way around.

Throttle and rudder are continuous in `[-1, 1]`. Inertia matters — you can't stop on a dime, and high-speed turns are wide. This is the entire skill curve of movement: learning to lead your own future position.

### 5.3 Sensors — The Key Tactical Choice

Each tick the bot picks **one** sensor mode:

**Active radar**
- Detects every ship and shell within **350 units**.
- Returns bearing **and** range, low noise (±2 units position).
- **Cost**: while active, this ship is detectable by *everyone* on the map (bearing only, no range), regardless of distance. You light up like a Christmas tree.

**Passive listening**
- Detects ships within **500 units** that were active last tick (you hear their radar).
- Detects ships within **150 units** unconditionally (engine noise).
- Returns **bearing only**, higher noise (±5°).
- You are not detectable from this.

This single binary choice — "do I see clearly but advertise myself, or stay quiet and guess?" — is where most of the tactical interest comes from. New players will run active 100% of the time. Better players learn to ping intermittently. The best players learn to track passively and only ping when committing to a shot.

### 5.4 Weapons

One gun, ballistic shells.

- **Fire command**: bearing (absolute, not relative) and a requested range.
- **Shell speed**: 70 units/s, constant.
- **Shell flight time** = `range / 70` seconds, capped at ~4.3 seconds (max range 300 units).
- During flight, shells are visible to active radar.
- On expiry: explode. Any ship within **15 units** of the splash takes damage based on proximity (linear falloff: 25 dmg at center, 0 at the edge).
- Gun cooldown enforced server-side; `fire` commands during cooldown are silently ignored (an `error` event is sent to the bot).
- Friendly fire is on (you can hit yourself). This punishes sloppy bearing math and makes the game more honest.

The "requested range" mechanic (instead of "shell flies until it hits something") makes leading targets a real skill: you must predict *where* and *when* the enemy will be, not just point at them.

### 5.5 Damage and Death

- HP starts at 100, ship destroyed at 0.
- A destroyed ship is removed from the world; its bot receives `game_over` (with `winner` set to whoever wins overall, not the killer).
- Last ship alive wins. If two ships die on the same tick and none remain, it's a draw.
- Hard timeout: if a match exceeds **3000 ticks** (5 minutes at 10 Hz), the ship with highest HP wins; tie-break by lowest ammo used.

### 5.6 Starting Conditions

For N players, ships are placed evenly on a circle of radius 400 around the map center, all facing the center. This guarantees symmetric, fair starts and makes 2/3/4-player matches all work without map redesign.

---

## 6. Determinism

Determinism is required for replays and for "same inputs → same outcome" fairness. Five rules:

1. **Fixed tick rate** — physics step is `dt = 0.1s` exactly, regardless of wall clock. The tick loop sleeps to align with wall clock but the simulation never reads it.
2. **Stable command ordering** — at each tick, sort received commands by `bot_id` before applying. Never apply in arrival order.
3. **Seeded RNG** — all randomness (sensor noise, starting positions if randomized, tie-breaking) draws from a single `rand_pcg::Pcg64` seeded from the room's seed. No `thread_rng`, no system time, no `HashMap` iteration order in simulation logic (use `BTreeMap` or sort keys).
4. **Float discipline** — pin to `f32` everywhere in the simulation, avoid transcendental functions in hot paths where possible, and never mix in `f64` partway through. For an MVP, plain `f32` math is fine; cross-platform replay between different CPUs is a known hard problem and out of scope.
5. **Replay log** — the server writes a JSONL file per match: header (seed, config, bot names) followed by one line per tick containing the sorted command list. Replaying = re-running the simulation with this log; spectator output should be byte-identical.

---

## 7. SDK Surface (Optional Convenience Layer)

Ship a thin Python SDK as the reference; everything else is "here's the protocol, go nuts."

```python
from naval_sdk import Bot, WorldView, Command, run

class MyBot(Bot):
    def on_welcome(self, info):
        self.map = info.map

    def on_tick(self, view: WorldView) -> Command:
        cmd = Command()
        cmd.throttle = 1.0
        cmd.rudder = 0.2
        cmd.sensor_mode = "active"

        # Helper: find nearest contact and shoot at it
        target = view.nearest_contact(kind="ship")
        if target and view.self.ammo > 0:
            cmd.fire_at(target.pos, lead=True)  # SDK computes lead based on shell speed
        return cmd

if __name__ == "__main__":
    run(MyBot, host="localhost", port=7878, name="captain_kirk")
```

The SDK should provide:
- Connection, handshake, reconnect, message loop.
- Typed view objects (`WorldView`, `Contact`, `SelfState`).
- Math helpers: `bearing_to(pos)`, `distance(a, b)`, `lead_target(target_pos, target_vel, shell_speed)`.
- A `run()` function that handles the boilerplate.

The SDK must **never hide the protocol**. Expose a `raw_send(dict)` escape hatch so power users can do things the SDK didn't anticipate.

---

## 8. Spectator Client

Single-page HTML + Canvas, no build step. Loads from `/` on the server (Rust serves a static file).

Renders:
- Map bounds.
- Each ship as a triangle, colored per player, with a name label and HP bar.
- Active radar: faint translucent circle around the ship while it's pinging.
- Shells: small dots with a faint trail.
- Splashes: 200ms expanding ring on hit/expire.
- Sidebar: tick counter, player list with HP/ammo, last 5 events.

Subscribes to `/spectate` and renders every received `world` message. No interpolation needed at 10 Hz for an MVP — it'll look fine.

---

## 9. Project Layout

```
naval-battle/
├── server/                 # Rust
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # CLI parsing, runtime startup
│       ├── net.rs          # WebSocket accept loop
│       ├── room.rs         # Room state machine
│       ├── sim/
│       │   ├── mod.rs
│       │   ├── world.rs    # World struct, ships, shells
│       │   ├── physics.rs  # Movement integration
│       │   ├── sensors.rs  # Active/passive view computation
│       │   └── combat.rs   # Firing, damage resolution
│       ├── protocol.rs     # serde types matching §4
│       ├── replay.rs       # JSONL log writer
│       └── control.rs      # stdin command parser
│
├── sdk-python/             # Reference SDK
│   ├── pyproject.toml
│   └── naval_sdk/
│       ├── __init__.py
│       ├── bot.py
│       ├── protocol.py
│       └── helpers.py
│
├── spectator/              # Static HTML/JS
│   ├── index.html
│   ├── render.js
│   └── style.css
│
├── examples/
│   ├── circle_bot.py       # Drives in a circle, fires randomly
│   ├── chaser_bot.py       # Active radar + naive pursuit
│   └── sniper_bot.py       # Passive + lead targeting
│
├── docs/
│   ├── PROTOCOL.md         # This protocol spec, standalone
│   └── QUICKSTART.md       # 5-minute hello-world
│
└── README.md
```

---

## 10. Build Order for the Hackathon

A suggested order, each step independently demoable:

1. **Server skeleton**: CLI parsing, accept WebSocket, echo messages. *Demo: `wscat` round-trip.*
2. **Protocol types**: serde structs for all messages in §4. *Demo: server validates and rejects malformed messages.*
3. **World + physics**: ships move with throttle/rudder, walls work, no combat. *Demo: spectator sees a triangle moving.*
4. **Single-bot loop**: one bot connects, sends commands, sees its own state. No sensors, no combat. *Demo: a Python script drives a ship in circles.*
5. **Multi-bot + sensors**: filtered views, active vs passive. *Demo: two bots, one cloaked, only the active one is detected.*
6. **Combat**: firing, shells, damage, death, win condition. *Demo: full match between two scripted bots.*
7. **Spectator UI**: canvas rendering. *Demo: watch a match in the browser.*
8. **Replay**: JSONL log + a `--replay <file>` flag that re-runs and re-broadcasts. *Demo: rewatch yesterday's match.*
9. **Polish**: SDK helpers, example bots, quickstart doc, error messages. *Demo: a teammate writes their first bot in 10 minutes.*

Steps 1–6 are the critical path. Everything after is "nice to have but can ship without."

---

## 11. Open Questions / Future Extensions

Things deliberately left out of MVP, easy to add later:

- **Terrain and islands** — turns the map from "plane" to "puzzle."
- **Torpedoes** — slower, smarter, finite-supply secondary weapon.
- **Team play** — 2v2 with shared sensor data among teammates.
- **Asymmetric ship classes** — destroyer (fast, fragile) vs battleship (slow, durable).
- **Energy budget** — radar pings cost energy, so spam isn't free.
- **Hosted bots** — sandboxed Docker containers for tournaments where players submit code instead of running clients.
- **Tournament mode** — bracket scheduler, automatic match orchestration.

None of these change the core architecture. The protocol, tick loop, and determinism story all carry over unchanged.

---

## 12. Summary

The core shape: **Rust server + WebSocket/JSON protocol + browser spectator + thin optional SDK**. The simulation is deterministic by construction, bots are remote and untrusted, and the operator runs the show via stdin. Gameplay reduces to three orthogonal decisions per tick — *where to go, what to see, when to shoot* — which is enough to produce real strategy without requiring weeks of design work.

For a hackathon: aim to land steps 1–6 in the build order, ship a Python example bot and a working spectator, and let the players' creativity carry the rest.