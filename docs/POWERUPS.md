# Powerups

Each bot picks **two distinct powerups** before a match starts and can activate each
one **once** during play for a time-bounded effect. Powerups tweak normal ship knobs
(speed, damage, sensor range…) so the strategic depth comes from *when* you fire them,
not from learning a new game.

This is the public reference. Keep it in lock-step with `server/src/sim/powerups.rs`,
`server/src/sim/constants.rs` (defaults), and `server/src/sim/config.rs`
(`PowerupConfig`).

## How to use them

### Selecting your loadout

After your bot receives `welcome` and before it sends `ready`, send:

```json
{ "type": "select_powerups", "powerups": ["rapid_fire", "heavy_shell"] }
```

Constraints (the server validates):

- Exactly two ids.
- Both ids distinct.
- Each id must appear in `welcome.available_powerups`.
- Selection is only accepted while the room is in `lobby`.

A bot that never sends `select_powerups` starts the match with no powerups — vanilla
play. An invalid loadout earns a typed `error` frame (one of the
`powerup_*` codes in `docs/PROTOCOL.md`) and the bot's *previous* selection (if any) is
kept; pick again if you want to retry.

### Activating in flight

Set `activate_powerup` on a normal `command` message:

```json
{
  "type": "command",
  "tick": 142,
  "throttle": 1.0,
  "rudder": 0.0,
  "sensor_mode": "active",
  "activate_powerup": "overdrive"
}
```

The activation resolves the same tick the command is applied — alongside `fire`,
*before* physics integrate. Each picked powerup can be activated at most once per match.
The server replies with a typed error if the powerup isn't in your loadout, was already
used, or your ship is dead.

### Checking status

Every `tick` payload echoes your loadout and live status:

```json
"self": {
  "selected_powerups": ["overdrive", "rapid_fire"],
  "powerup_status": [
    { "id": "overdrive",  "used": false, "active_ticks_left": 0 },
    { "id": "rapid_fire", "used": true,  "active_ticks_left": 23 }
  ]
}
```

The Python SDK exposes this as `view.me.powerup_ready("...")` and
`view.me.powerup_active("...")`. The `examples/loadout_bot.py` reference bot
demonstrates the full workflow.

## The catalog

All durations are in *ticks* (default tick rate is 10 Hz, so `50 ticks = 5 s`).

### Movement & Defense

#### `overdrive`
Boosts your max forward speed, acceleration, and turn rate. Pure mobility burst —
great for closing the gap, dodging, or repositioning after dropping smoke.

- Duration: **50 ticks**
- Max speed × 1.6 · Acceleration × 1.6 · Turn rate × 1.5
- Synergy: `smoke_screen` (lay smoke, dash through it), `decoy_flare` (slip away while
  the enemy chases the phantom).
- Counter to: `emp_burst` (outrun the AoE), `long_range_salvo` (close the distance fast).

#### `reinforced_hull`
Tank up. Incoming splash damage is multiplied by 0.45 for the window. Slightly weaker
per hit than before, but a longer window gives reactive picks more timing slack.

- Duration: **70 ticks**
- Damage multiplier: **0.45**
- Synergy: `repair_drones` (tank build that out-survives a sustained burst).
- Counters: `heavy_shell`, `rapid_fire`.

#### `repair_drones`
Front-loaded repair: an instant **+20 HP** the moment you activate, then **+1 HP per
tick** for the window (everything capped at `hull_hp`). The burst is the point — regen
alone can never answer a concentrated salvo, but the instant chunk can.

- Instant heal: **+20 HP** · Duration: **50 ticks** (+1 HP/tick → ~70 HP total, capped)
- Synergy: `reinforced_hull`.
- Countered by: `rapid_fire` + `heavy_shell` (a big enough single burst still out-damages it).

#### `smoke_screen`
Drops a static smoke cloud at your current position. Ships *inside* the cloud are
invisible to active radar from viewers *outside* it. Passive sensors still pick up
engine noise (unless paired with `silent_running`).

- Cloud radius: **70 units** · Duration: **80 ticks**
- Synergy: `silent_running` (real stealth bubble), `overdrive` (place + dash).
- Counter to: `long_range_salvo` (denies the snipe), partial counter to `awacs_scan`.

### Offense

#### `rapid_fire`
Gun cooldown is multiplied by 0.5 for the window — roughly double the rate of fire.

- Duration: **50 ticks** · Cooldown multiplier: **0.5**
- Synergy: `heavy_shell` (volume × power), `awacs_scan` (find then unload),
  `emp_burst` (lock targets then beat them).
- Countered by: `reinforced_hull`, `repair_drones`, `silent_running`.

> **Rounding:** cooldown ticks are rounded to the nearest integer, ties rounding up. With the
> default 15-tick cooldown, `round(15 × 0.5) = round(7.5) = 8` ticks.

#### `heavy_shell`
Shells *fired during the next 30 ticks* get **× 1.5 splash radius** and **× 1.3 max
splash damage**. The buff travels with the projectile — a shell fired right before
expiry still detonates buffed. (×1.5 radius still ~2.25× the splash *area*, so it stays
punishing without trivializing aim.)

- Duration (buff window): **30 ticks**
- Synergy: `rapid_fire`.
- Countered by: `reinforced_hull`, `silent_running` (no target).

#### `long_range_salvo`
Shells *fired during the next 40 ticks* get **× 1.5 max range** (450 u) and **× 1.6 shell
speed** (112 u/s). Flight time, not reach, is the real limiter, so the buff leans into
speed: 450 u at 112 u/s ≈ 4.0 s to target.

- Duration (buff window): **40 ticks**
- Synergy: `awacs_scan` (see far + shoot far).
- Counter to: `silent_running` (catch them at range).
- Countered by: `smoke_screen`, `counter_battery_trace`.

### Sensors & Information

#### `awacs_scan`
Active radar range × 2.0 and noise drops to 0 on normal contacts for the window. A
**soft** counter to `silent_running`: a silent runner is *not* fully exposed — it shows
up only within your **base** radar range (350 u, not the doubled range), and only as a
jittered (±15 u), low-confidence (0.6) contact. Enough to know something's there; not a
clean fire solution.

- Duration: **60 ticks** · Range multiplier: **2.0** (normal contacts)
- Silent-runner detection: within **base range** only · jitter **±15 u** · confidence **0.6**
- Synergy: `long_range_salvo`, `rapid_fire`.
- Counter to: `silent_running` (soft — degrades, doesn't defeat it).
- Countered by: `decoy_flare`.

#### `silent_running`
Vanish from passive sensors entirely, and any active radar against you has its
effective range halved. **Firing your gun breaks `silent_running` immediately** — the
muzzle flash is unambiguous.

- Duration: **80 ticks** · Active range against you: **× 0.5**
- Synergy: `smoke_screen` (compound stealth), `counter_battery_trace` (hide, then trace
  the next attacker).
- Countered by: `awacs_scan`.

#### `counter_battery_trace`
Arms a **non-consuming** trace for the window. *Every* hit that lands on you while armed
(re)starts a 15-tick full-confidence track on whoever fired it — so sustained aggression
keeps the attacker lit, not just the first shot. The synthetic contact carries a
`cbt_<n>` id so you can tell it apart from a normal sensor return.

- Arm window: **60 ticks** · Reveal track per hit: **15 ticks** (refreshes on each hit)
- Counter to: `long_range_salvo`, sniper plays via `silent_running`.
- Countered by: `decoy_flare` (the attacker may have been faking with friends nearby).

### Disruption

#### `emp_burst`
Instantaneous AoE centered on you. All *enemy* ships within 130 u get their gun
cooldown × 2 and their active radar disabled (returns empty contacts) for the window.
Wider but shorter than before — 100 u was only a third of weapon range, too conditional.

- Radius: **130 units** · Duration: **40 ticks** · Cooldown × **2.0**
- Synergy: `rapid_fire` (lock them, then beat them).
- Counter to: `rapid_fire` opponents, `awacs_scan` opponents.
- Countered by: `overdrive` (outrun the AoE).

#### `decoy_flare`
Spawns a phantom contact ahead of you (along your current heading) at a seeded-jittered
distance of **80–140 u**. The phantom **inherits your heading and speed at spawn and
cruises** — a motionless contact with no velocity history is trivially filtered by a
tracker-grade bot, so the decoy moves. It shows up in everyone else's active radar and
passive sensors as if it were a real ship; you do not see your own decoy.

- Spawn distance ahead: **80–140 units** (seeded jitter) · cruises at your spawn velocity · Duration: **60 ticks**
- Synergy: `silent_running` (the real ship hides while the fake draws fire), `overdrive`.
- Counter to: `awacs_scan`, `counter_battery_trace`.

## Operator tuning

Every duration and multiplier above lives on `SimConfig.powerups` (a `PowerupConfig`
sub-struct). Operators can edit them via the existing `PUT /api/room/config` route
before a match starts; the values are frozen into the replay header so a recorded
match always replays with the parameters it ran with.

## Determinism

Powerup activation is part of the per-tick `command` payload, so it replays exactly
like `fire`. Every effect is keyed to `world.tick`; smoke clouds and decoys are
garbage-collected by `powerups::step_tick_maintenance` once their `expires_at` passes.
See `CLAUDE.md` for the broader determinism contract.
