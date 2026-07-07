# Review Findings — 2026-07-02

Prioritized findings from a full-application review (server, Python SDK, spectator,
docs). Every finding was verified against the code at the commit this document was
added; line numbers refer to that commit and may drift — re-locate by the quoted
identifiers if they do.

## How to use this document

This document is written so a coding agent can pick up any single finding and fix it
without re-deriving context.

- **Work phase by phase, top to bottom.** Phases are ordered by priority. Within a
  phase, findings are independent of each other unless a **Depends on** line says
  otherwise — pick any unclaimed one.
- **One finding = one commit** (or one small commit series). Reference the finding ID
  (e.g. `F-01`) in the commit message. Tick the checkbox in the tracking table in the
  same commit.
- **Read `CLAUDE.md` first.** Several findings touch the determinism contract, the
  sensor filter, or the wire protocol — the invariants there override any shortcut.
- **Protocol sync rule:** any finding marked *Protocol sync: yes* must update all four
  protocol surfaces together: `server/src/protocol.rs`, `docs/PROTOCOL.md`,
  `sdk-python/naval_sdk/protocol.py`, `spectator/src/types/protocol.ts`.
- **Before committing:** `cargo fmt && cargo clippy --all-targets -- -D warnings &&
  cargo test` (in `server/`), `pytest` (in `sdk-python/`), `npm test` (in
  `spectator/`). If you touched `spectator/src/`, run `npm run build` and commit the
  regenerated `dist/` in the same commit.

### Global note: sim-behavior fixes invalidate old replay logs

F-09, F-17, F-18, F-19 (and F-20 if implemented rather than removed) change simulation
behavior. Replay files recorded before such a fix will no longer re-simulate
byte-identically. That is acceptable — but batch these fixes where practical, bump
`REPLAY_FORMAT_VERSION` if the log format itself changes (F-02 does), and note
behavior changes in `docs/PROTOCOL.md`'s changelog.

## Tracking table

| ID | Phase | Title | Severity | Component | Status |
|------|---|---------------------------------------------------------|----------|-----------|--------|
| F-01 | 1 | Replay applies commands early on tick gaps | Critical | server | [x] |
| F-02 | 1 | Mid-match disconnects missing from replay log | Critical | server | [x] |
| F-03 | 1 | Operator abort wedges Monte Carlo runs | Critical | server | [x] |
| F-04 | 2 | SDK loses powerup loadout after first match | High | sdk | [x] |
| F-05 | 2 | Gunner cooldown carries across matches | High | sdk | [x] |
| F-06 | 2 | Malformed `game_start` crashes SDK run loop | High | sdk | [x] |
| F-07 | 3 | Lockstep MC still enforces wall-clock deadline | Medium | server | [x] |
| F-08 | 3 | `PowerupActivated` leaks ground-truth ShipId | Medium | server | [x] |
| F-09 | 3 | Unclamped turn-rate ratio (Overdrive super-turn) | Medium | server | [ ] |
| F-10 | 3 | Replay ID collision truncates previous replay | Medium | server | [ ] |
| F-11 | 3 | Replay endpoints unbounded on corrupt logs | Medium | server | [ ] |
| F-12 | 3 | EMP debuff misreported as own powerup active | Medium | server | [ ] |
| F-13 | 4 | MC panel dead-ends after a completed run | Medium | spectator | [ ] |
| F-14 | 4 | Live view hardcodes 1000×1000 map | Medium | spectator | [ ] |
| F-15 | 4 | Radar ring ignores tunable `active_radar_range` | Medium | spectator | [ ] |
| F-16 | 4 | Replay perspective switch shows mismatched data | Medium | spectator | [ ] |
| F-17 | 5 | Sensor-side powerup windows one tick short | Low | server | [ ] |
| F-18 | 5 | `wrap_deg` can emit heading exactly 360.0 | Low | server | [ ] |
| F-19 | 5 | Decoys trivially fingerprintable | Low | server | [ ] |
| F-20 | 5 | `hit_radius` published but dead | Low | server | [ ] |
| F-21 | 6 | powerful_bot velocity filter halves true velocity | Medium | examples | [ ] |
| F-22 | 6 | `Welcome` not exported; README StatBot crashes | Low | sdk | [ ] |
| F-23 | 6 | SDK `run()` raises on server close mid-send | Low | sdk | [ ] |
| F-24 | 6 | "Up to 2" vs "exactly 2" powerup docs conflict | Low | sdk/server | [ ] |
| F-25 | 7 | PROTOCOL.md documents wrong MC seed formula | Low | docs | [ ] |
| F-26 | 7 | Spectator TS types missing server fields | Low | spectator | [ ] |
| F-27 | 7 | system-design.md wire examples drifted | Low | docs | [ ] |
| F-28 | 7 | SDK README documents wrong default map size | Low | docs | [ ] |
| F-29 | 7 | Dead `connection_limit` error code | Low | server | [ ] |
| F-30 | 7 | `tokio-tungstenite` in prod deps, used only by tests | Low | server | [ ] |
| F-31 | 7 | Violation-disconnect test passes if server crashes | Low | server | [ ] |
| F-32 | 7 | Admin broadcast channel never wired in production | Low | server | [ ] |
| F-33 | 7 | MC run permanently mutates room seed and config | Low | server | [ ] |
| F-34 | 7 | Spectator fallback constants drifted from server | Low | spectator | [ ] |

---

## Phase 1 — Determinism & replay contract (do these first)

These violate the project's core promise: bit-identical replays. All three are
invisible to the current test suite because the tests use bots that command every
tick and never disconnect.

### F-01 — Replay applies commands too early whenever there is a tick gap

- **Severity:** Critical. **Effort:** S. **Protocol sync:** no (replay format unchanged).
- **Files:** `server/src/replay.rs` (`run_replay` ~530–546, `capture_replay` ~608–631,
  `capture_perspective` ~716–740), `server/src/room.rs` (~616–621).

**Problem.** All three replay drivers do: inject commands for record `rec`, then
`while room.world.tick < rec.tick { room.step_tick(); }`. But `step_tick` drains
`pending_command.take()` unconditionally at the top of *every* step — the
`PendingCommand.tick` field does not gate consumption. The writer skips empty ticks
(see the comment at `replay.rs:541`), so if the previous record was tick 100 and the
next is tick 105, the tick-105 commands are consumed by the step that produces tick
**101** — four ticks early. Live and replayed trajectories diverge for any match in
which any bot skipped a tick (routine: `late_command` rejections, slow bots).

**Fix.** In all three loops, step to `rec.tick - 1` first, then inject, then step
once:

```rust
while room.world.tick < rec.tick - 1 { room.step_tick(); }
for cmd in rec.commands { room.inject_replay_command(...); }
room.step_tick(); // produces rec.tick, consuming the injected commands
```

Watch the edge case of the first record (world starts below `rec.tick - 1` already
holds) and keep the `End` record handling as-is. Apply identically in `run_replay`,
`capture_replay`, and `capture_perspective` — they must share semantics (consider
extracting a helper so they can't drift again).

**Verify.** New test in `server/tests/replay_determinism.rs`: a scripted bot that
commands only every 3rd tick (and one that goes silent for 10 ticks mid-match).
Assert final world state is byte-identical between live run and replay. This test
must fail before the fix and pass after.

### F-02 — Mid-match disconnects/kicks are not recorded in the replay log

- **Severity:** Critical. **Effort:** M. **Protocol sync:** partial — replay format
  bump + `docs/PROTOCOL.md` §2.6 + `spectator/src/types/protocol.ts` if the captured
  replay JSON surfaces it.
- **Files:** `server/src/room.rs` (`handle_bot_disconnect` ~1217–1227,
  `OperatorKick` handler ~1623), `server/src/replay.rs` (`ReplayRecord` ~62–68,
  `rebuild_room_with_outbound` ~346, all three replay drivers).
- **Depends on:** F-01 (fix injection semantics first so the new record type inherits
  correct timing).

**Problem.** A mid-match disconnect (or operator kick) removes the bot and its ship
from the world immediately, in any room state. The replay log has only
`Header | Tick | End` records, and replay rebuild registers every header bot and never
removes any. Live: the ship vanishes → other bots' sensor sweeps draw fewer RNG values
→ the shared Pcg64 stream shifts → the match may end early. Replay: a ghost ship
persists, consuming different RNG draws. The re-simulated final state can contradict
the recorded `End`. Bots crashing mid-match is the *common* case at a hackathon.

**Fix.** Add `ReplayRecord::Disconnect { tick, bot_id }`. Bump
`REPLAY_FORMAT_VERSION` 4 → 5. Writer: emit the record from the disconnect/kick path
while `Running`, using the same post-bump tick convention as command records (define
precisely: the record's `tick` is the first tick at which the ship no longer exists).
Replay: apply the removal at that exact point relative to `step_tick`, mirroring the
live call into `handle_bot_disconnect`'s world-mutation (factor the ship-removal part
out of the network-facing handler so replay can call it without a channel). Keep v4
logs readable (they just predate the record type).

**Verify.** New replay test: 2-bot match, forcibly remove one bot mid-match, run to
completion, assert replay reproduces the byte-identical final state and the recorded
winner. Also assert a v4 log still loads.

### F-03 — Operator abort during a Monte Carlo run wedges the room

- **Severity:** Critical. **Effort:** S.
- **Files:** `server/src/room.rs` (`OperatorAbort` handler ~1607–1614, `abort_match`
  ~1143–1154, post-game gate ~588, MC chain ~717, `transition_to_lobby` ~1171,
  `mc_abort` ~2228).

**Problem.** `OperatorAbort` calls `abort_match()` directly, which sets
`state = Ended` but never touches `mc_run`. The post-game auto-return to lobby is
gated on `mc_run.is_none()` (~588) and MC chaining only runs from the `Running`
match-outcome branch (~717) — so the room sits in `Ended` forever with
`/api/montecarlo/status` reporting `running: true`. Worse: `OperatorReset` succeeds
and `transition_to_lobby` does **not** clear `mc_run`, so the next *normal* match's
result is recorded into the stale MC run, which then chains leftover MC matches with
MC-derived seeds.

**Fix.** In the `OperatorAbort` handler (or inside `abort_match`): if
`mc_run.is_some()`, call `mc_abort("operator_abort")` (it already ends the in-flight
match and publishes final status). In `transition_to_lobby`, `debug_assert!` that
`mc_run` is `None` — and clear it defensively with a `warn!` if not.

**Verify.** Unit tests in `room.rs`: (a) abort mid-MC-run → status shows
`running: false` with reason, room auto-returns to lobby after the pause; (b) abort +
reset + start a normal match → match report is normal, no MC chaining occurs.

---

## Phase 2 — SDK bugs every bot author hits

### F-04 — SDK bots lose their powerup loadout after the first match

- **Severity:** High. **Effort:** S.
- **Files:** `sdk-python/naval_sdk/bot.py` (`welcome` branch ~196–208, `lobby` branch
  ~257–266). Server behavior (correct, don't change): `server/src/room.rs` ~1187,
  ~1194 clears loadouts on return to lobby; `docs/PROTOCOL.md` lifecycle §
  (`(lobby) → [select_powerups] → ready → game_start`).

**Problem.** The SDK sends `select_powerups` only once per connection, gated by
`ready_sent` inside the `welcome` branch. On the `lobby` message it only re-sends
`ready`. The server drops committed loadouts every time the room returns to lobby, so
any bot overriding `choose_powerups` plays match 2+ with no powerups.

**Fix.** Extract the "choose powerups (if any) then send ready" sequence into a
helper; call it from both the `welcome` branch and the `lobby` branch. Keep a
per-lobby guard so a repeated `lobby` frame doesn't double-send. `choose_powerups`
needs the `Welcome` — reuse `bot.welcome` (already stored).

**Verify.** Extend `sdk-python/tests/test_multiround.py` with a scripted server that
sends `welcome → lobby → game_start → game_over → lobby`; assert `select_powerups` is
sent after **each** lobby, with the same picks, before each `ready`.

### F-05 — Gunner cooldown carries across match resets

- **Severity:** High. **Effort:** S.
- **Files:** `sdk-python/naval_sdk/tactical/gunner.py` (~62, `note_fired`),
  `sdk-python/naval_sdk/tactical/bot.py` (`on_game_start` ~95–107),
  `examples/tracking_bot.py` (~38–44), `examples/tactician_bot.py` (~60–67).

**Problem.** `Gunner.solve` gates on absolute ticks (`view.tick <
self._next_fire_tick`). Server ticks reset to 0 each match. `TacticalBot.on_game_start`
deliberately skips resetting the gunner with a comment whose reasoning is inverted
("a reset to tick 0 makes it immediately fireable again" — no: tick 0 < the stale
`_next_fire_tick`). After firing late in match 1 (e.g. `note_fired(2990)` →
`_next_fire_tick = 3005`), the gunner refuses every shot in match 2 until tick 3005 —
effectively the whole match. `Gunner` has no `reset()` at all (Tracker and Evader
both have one), and both examples reproduce the gap.

**Fix.** Add `Gunner.reset()` setting `_next_fire_tick = 0`. Call it from
`TacticalBot.on_game_start` and delete the wrong comment. Update both examples to
call it in their `on_game_start`.

**Verify.** Test in `test_multiround.py`: `note_fired(2990)`, then simulate a new
match at tick 0 → `solve` must return a solution (given a valid track).

### F-06 — Malformed `game_start` crashes the SDK run loop

- **Severity:** High. **Effort:** S.
- **Files:** `sdk-python/naval_sdk/bot.py` (`game_start` branch ~211–222).

**Problem.** The `try` validates `msg["tick"]` and `msg["starting_heading_deg"]` but
only *binds* `pos = msg["starting_position"]`. The indexing
`(float(pos[0]), float(pos[1]))` is evaluated at the `_safe_callback` call site,
**outside** the `try`. A frame with `"starting_position": 42` raises `TypeError`
straight out of `run_async`, killing the bot — violating the CLAUDE.md rule that the
SDK never dies on a malformed server message.

**Fix.** Convert the position inside the `try`:
`start_pos = (float(pos[0]), float(pos[1]))` next to the other conversions, then pass
`start_pos` to `_safe_callback`.

**Verify.** Test: feed `game_start` frames with `starting_position` = `42`, `[]`,
`["x","y"]`, missing — assert the loop logs and continues, then processes a
subsequent valid frame.

---

## Phase 3 — Server gameplay & robustness

### F-07 — Lockstep Monte Carlo still enforces the wall-clock `tick_deadline_ms`

- **Severity:** Medium. **Effort:** S.
- **Files:** `server/src/room.rs` (`handle_bot_command` ~1797–1826, lockstep wait in
  `run_room` ~2589–2637, MC match boundary ~2166–2205), `server/src/monte_carlo.rs`
  (`lockstep_timeout` ~34, module doc ~7–9).

**Problem.** The `late_command` wall-clock rejection runs whenever
`state == Running`, with no lockstep exemption. A bot slower than `tick_deadline_ms`
(default 80 ms) gets *every* command rejected in MC mode; `pending_command` stays
`None`; `all_pending_commands_ready()` is never true; every tick burns the full 1 s
lockstep timeout with ships drifting on stale controls. This defeats lockstep's
stated purpose ("the match goes as fast as the slowest bot can respond").

**Fix.** Skip the wall-clock deadline check when the room is in lockstep mode (the
room already knows: see `is_lockstep()`-style accessor near ~2279). Keep the
stale-tick window check (it's tick-based, not wall-clock). While there, check the
match-boundary behavior (~2166–2205): the first tick of each chained match currently
tends to wait out the full timeout because in-flight commands echo the old tick —
consider widening the stale-tick acceptance window for tick 0/1 of a new match, or
document why not.

**Verify.** Extend `server/tests/monte_carlo_determinism.rs` (or add a room unit
test): in lockstep mode, a command arriving 200 ms after the tick frame is accepted
and applied next step; batch throughput with a slow scripted bot is bounded by the
bot, not by `lockstep_timeout` per tick.

### F-08 — `PowerupActivated` leaks ground-truth `ShipId` and over-reveals

- **Severity:** Medium. **Effort:** M. **Protocol sync:** yes.
- **Files:** `server/src/room.rs` (event emission ~771–780, `is_ship_visible_to`
  ~1405–1473, contact anonymization `translate_contact` ~2496),
  `server/src/sim/sensors.rs` (~79, AWACS soft-counter), `server/src/protocol.rs`
  (~203–218).

**Problem.** Sensor contacts are deliberately anonymized per tick (`c_<index>`), but
`TickEvent::PowerupActivated` hands bots the persistent ground-truth `ShipId`,
letting them re-identify and track a specific opponent for the whole match. Its
visibility gate is also a parallel reimplementation that is *stronger* than the real
sensor model (treats AWACS as a hard counter to silent running; `sensors.rs` says
soft), so bots get activation events on ticks where their sensor sweep showed
nothing. `protocol.rs` claims the event is "subject to the same filtering as ship
contacts" — it isn't.

**Fix.** Two parts. (1) Replace `ship_id` in the bot-facing event with either the
viewer's anonymized contact id for that ship on that tick (when visible in the same
sweep) or a coarse indicator (e.g. omit the id entirely and keep only `powerup`) —
pick one, document it. (2) Gate the event on the *actual* sensor result for that
viewer/tick instead of `is_ship_visible_to`'s parallel rules, or align
`is_ship_visible_to` with `sensors.rs` (soft counters stay soft). Spectator payload
keeps ground truth — only the bot-facing event changes. This is a breaking protocol
change: follow the 4-place sync rule and bump per `docs/PROTOCOL.md` changelog rules.

**Verify.** Room unit test: bot A under silent running + enemy AWACS, A activates a
powerup on a tick where B's sweep returns no contact → B receives no
`powerup_activated` (or an anonymized one, per chosen design). Grep the bot-facing
payload path for remaining ground-truth `ShipId`s.

### F-09 — Unclamped turn-rate speed ratio (Overdrive expiry super-turn)

- **Severity:** Medium. **Effort:** S. **Behavior change:** invalidates old replays
  (see global note).
- **Files:** `server/src/sim/physics.rs` (~64).

**Problem.** `turn_rate = turn_rate_max * ship.rudder * (ship.speed.abs() /
max_forward)` — the ratio is never clamped. When Overdrive expires while the ship is
still at boosted speed (14.4 vs base max 9), ratio = 1.6 → 32°/s for ~15 ticks,
faster than Overdrive itself grants (30°/s). Exploitable "super-turn on expiry".
Conversely, activating Overdrive momentarily *reduces* turn rate (multiplier 1.5 on
the numerator vs 1.6 on the denominator), contradicting `docs/POWERUPS.md`.

**Fix.** Clamp: `(ship.speed.abs() / max_forward).min(1.0)`. Note the
activation-moment dip is inherent to scaling by *current* max; if the dip should also
go, that's a balance discussion — ask first (per CLAUDE.md "changing physics
constants"). The clamp alone is a bug fix.

**Verify.** Unit test in `physics.rs`: ship at speed > `max_forward` turns at exactly
`turn_rate_max * rudder`, never more.

### F-10 — Replay ID collision silently truncates the previous replay

- **Severity:** Medium. **Effort:** S.
- **Files:** `server/src/replay.rs` (`make_replay_id` ~316–322, `File::create` ~208),
  `server/src/room.rs` (~1950–1953).

**Problem.** Non-MC replay IDs are `match_{room}_{unix_secs}` — 1-second resolution —
and the writer opens with `File::create`, which truncates. Start → abort → reset →
start within one wall-clock second (trivial via the REST API) destroys the first
match's replay with no warning.

**Fix.** Disambiguate: add a per-process monotonic counter to the id (preferred —
keeps determinism rules; do **not** reach for wall-clock in sim code, this is
room/replay layer so millis are acceptable too), and/or open with `create_new` and
retry with a `_2` suffix on collision.

**Verify.** Unit test: two consecutive `make_replay_id` calls in the same second
yield distinct ids; writer refuses to truncate an existing file.

### F-11 — Public replay endpoints unbounded on corrupt logs

- **Severity:** Medium. **Effort:** S.
- **Files:** `server/src/net.rs` (~642–687), `server/src/replay.rs` (`capture_replay`
  ~627–631, `capture_perspective` ~716–740), `server/src/room.rs` (tick keeps
  incrementing after end, ~597–601).

**Problem.** `GET /api/replays/:id` and `/perspective/:bot_id` re-simulate the whole
match per request. The step loops trust `rec.tick` from the file: a truncated/corrupt
log whose `End` record says `tick: 10^12` steps a trillion ticks and allocates one
frame per tick — a hung `spawn_blocking` thread and OOM from a merely corrupt
on-disk file.

**Fix.** Sanity-cap the step loops: derive a ceiling from the header's config (match
timeout ticks + margin) or a hard constant (e.g. 1,000,000 steps); exceeding it
returns a 422 `invalid_replay` error, not a 500. Optionally cache the last captured
replay by id (cheap win; not required).

**Verify.** Test: hand-craft a log with `End { tick: u64::MAX }` → endpoint returns
422 quickly. Existing valid-replay tests still pass.

### F-12 — EMP debuff misreported as the victim's own powerup being active

- **Severity:** Medium. **Effort:** M. **Protocol sync:** no wire-shape change, but
  update `docs/POWERUPS.md` / `docs/PROTOCOL.md` wording.
- **Files:** `server/src/sim/powerups.rs` (`emp_expires_at` dual use ~136,
  `is_active`/`ticks_remaining` ~171), `server/src/room.rs` (status build ~785–794).

**Problem.** One field stores both "my EmpBurst is running" and "I have been EMP'd by
an enemy". Bot A holds an unused `emp_burst`; enemy B EMPs A; A's `powerup_status`
now shows `{ "id": "emp_burst", "used": false, "active_ticks_left": 40 }` — the SDK's
`powerup_active("emp_burst")` reports A's own unused powerup as active.

**Fix.** Split into two fields (e.g. `emp_self_expires_at` for own activation,
`emp_debuff_until` for being hit). `powerup_status` reads only the self field; the
sensor/gun debuff paths read the debuff field. Audit every reader of the old field.
Note the world-state split affects replay byte-identity only if serialized state
changes — keep serialization shape stable or note it.

**Verify.** Unit test: victim with unused `emp_burst` gets EMP'd → its
`powerup_status` shows `used: false` with no `active_ticks_left`; its radar is still
suppressed for the debuff window; the attacker's own status shows active correctly.

---

## Phase 4 — Spectator (demo surface)

### F-13 — Monte Carlo panel dead-ends after a completed run

- **Severity:** Medium. **Effort:** S.
- **Files:** `spectator/src/components/MonteCarloPanel.svelte` (phase derivation
  ~38–43, "New run" button ~455–461).

**Problem.** Phase is derived purely from server status; after a batch completes,
`status.completed > 0` pins the panel to `completed` forever (server keeps reporting
the last run). "New run" does `appMode.set('monte-carlo')` — already the current
mode, a no-op. There is no way to start a second batch from the UI, even after
reload.

**Fix.** Add local state, e.g. `let showSetup = $state(false)`; "New run" sets it
true; phase derivation returns `'setup'` when `showSetup` and no run is `running`;
reset `showSetup` when a run starts.

**Verify.** Vitest on extracted phase logic if practical; manually: complete a run
(or mock `mcStatus`), click "New run" → setup form renders, starting a run returns to
the running view.

### F-14 — Live battlefield hardcodes a 1000×1000 map

- **Severity:** Medium. **Effort:** M. **Protocol sync:** yes (additive REST field).
- **Files:** `spectator/src/components/Battlefield.svelte` (~53),
  `spectator/src/lib/constants.ts` (~5–6), `server/src/net.rs` (`RoomResponse`
  ~343), reference implementation: `ReplayCanvas.svelte` (~39–40) already uses the
  replay header's map size.

**Problem.** The live view draws bounds and the letterbox transform for a hardcoded
1000×1000, but the server accepts `--map WxH` (default is 700×700). On any other map
ships render outside the drawn bounds and scaling is wrong. `GET /api/room` does not
currently expose map size.

**Fix.** Add `map: { width, height }` to `RoomResponse` (additive — document in
`docs/PROTOCOL.md` §2.5 and add to `spectator/src/types/protocol.ts`). Thread it from
the room-config store into `draw(...)`, falling back to constants only before the
first fetch. Follow the `hull_hp`/`maxHp` threading pattern already in place.

**Verify.** Run server with `--map 900x500`, open live view: bounds match, ships stay
inside. `npm test` green; rebuild `dist/` and commit it.

### F-15 — Active-radar ring ignores the tunable `active_radar_range`

- **Severity:** Medium. **Effort:** S.
- **Files:** `spectator/src/lib/renderer.ts` (~61, ~301),
  `spectator/src/lib/constants.ts` (~8), `spectator/src/components/Battlefield.svelte`
  (room config already subscribed, ~27).
- **Depends on:** none (can share plumbing with F-14).

**Problem.** Radar rings are drawn at a hardcoded 350 units while
`active_radar_range` is operator-tunable in the pre-match config form. The demo
surface then misrepresents what bots can actually see.

**Fix.** Thread `active_radar_range` from the subscribed room config into `draw()`
(same pattern as `maxHp`), constant as fallback only.

**Verify.** Set radar range to 200 via the config form; rings shrink. Rebuild
`dist/`, commit.

### F-16 — Replay perspective switch shows mismatched data

- **Severity:** Medium. **Effort:** S.
- **Files:** `spectator/src/stores/replay.ts` (`selectPerspective` ~59–77,
  `openReplay`), `spectator/src/components/ReplayCanvas.svelte` (~44–58).

**Problem.** Two races. (1) `selectPerspective` sets the perspective id immediately
but doesn't clear `replayPerspectiveData`, so while bot B's timeline loads the canvas
draws B's ship with A's contacts. (2) A perspective fetch still in flight when
`openReplay()` loads a *different* replay resolves late and writes into the freshly
cleared cache — poisoning it with the previous replay's data; the bot-id guard can
pass when both replays contain the same bot id.

**Fix.** Null `replayPerspectiveData` before fetching. Tag each in-flight fetch with
the replay id (or an epoch counter incremented by `openReplay`) and drop stale
resolutions in both the cache write and the store write.

**Verify.** Vitest on the store: switch perspectives with delayed fake fetches —
no stale frame is ever exposed; open replay B while A's fetch is in flight — cache
for B never contains A's data.

---

## Phase 5 — Sim polish (batch together; each invalidates old replays)

### F-17 — Sensor-side powerup windows are one tick shorter than documented

- **Severity:** Low. **Effort:** M.
- **Files:** `server/src/room.rs` (tick bump before sensor pass, ~678–692; reveal
  check ~1355), `server/src/sim/sensors.rs` (AWACS ~63–65, EMP blackout ~57–61,
  smoke ~167–182), `server/src/sim/combat.rs` (~184–185), `docs/POWERUPS.md`.

**Problem.** Effects use `expires_at = activation_tick + duration`, checked as
`expires > tick`. Physics/combat read the pre-bump tick and get the full N ticks; the
sensor pass runs after the bump and gets N−1: AWACS 59/60 boosted sweeps, smoke
79/80, EMP blackout 39/40, counter-battery reveal 14/15.

**Fix.** Pick one: (a) make the sensor-side checks use the same tick basis as
physics (e.g. compare against `tick - 1` or set `expires_at` with `+ duration + 1`
for sensor-scoped effects — be explicit and comment the convention), or (b) keep the
behavior and correct `docs/POWERUPS.md` to state sweep counts. (a) is preferred:
docs promise N. Whichever you choose, encode the convention in a comment at the
`expires_at` definition site.

**Verify.** Unit tests counting affected sweeps for each of the four effects — must
equal the documented durations.

### F-18 — `wrap_deg` can return exactly 360.0

- **Severity:** Low. **Effort:** XS.
- **Files:** `server/src/sim/physics.rs` (~99–107), invariant doc
  `server/src/sim/world.rs` (~25).

**Problem.** The guard handles `m < 0.0`, but f32 `rem_euclid` never returns
negative — it returns exactly `360.0` for tiny negative inputs (verified:
`(-1e-7f32).rem_euclid(360.0) == 360.0`). `heading_deg: 360.0` can leak to the wire,
violating the documented `[0, 360)`.

**Fix.** Replace the guard with `if m >= 360.0 { m - 360.0 } else { m }` and fix the
comment.

**Verify.** Unit test: `wrap_deg(-1e-7)` ∈ `[0, 360)`; property-style loop over a few
thousand values asserting the range.

### F-19 — Decoys are trivially fingerprintable

- **Severity:** Low. **Effort:** S.
- **Files:** `server/src/sim/sensors.rs` (decoy contacts, ~141–159),
  `server/src/sim/powerups.rs` (`activate` ~311–326, drift ~397–399),
  `docs/POWERUPS.md` (~198–199).

**Problem.** Docs promise the phantom appears "as if it were a real ship", but (a)
decoy contacts have zero position noise while real contacts jitter ±2u per sweep, and
(b) decoy positions are never clamped to the arena, so one launched near a wall
cruises outside `[0,W]×[0,H]` where no real ship can be. Two trivial filters defeat
the powerup.

**Fix.** (a) Apply the same noise draw to decoy contacts as to real ships —
**determinism caution:** this adds RNG draws; keep the draw order stable (decoys are
already iterated in deterministic order) and note that it invalidates old replays.
(b) Clamp decoy spawn and drift to the arena bounds, same as ships.

**Verify.** Unit tests: decoy contact positions vary across sweeps with the seeded
RNG; decoy never leaves bounds. Replay test still byte-identical for newly recorded
logs.

### F-20 — `hit_radius` is published, tunable, and dead

- **Severity:** Low. **Effort:** decision needed — **ask the maintainer first.**
- **Files:** `server/src/sim/constants.rs` (~22), `server/src/sim/config.rs`
  (~30/344/370), `server/src/protocol.rs` (~125/145), `server/src/net.rs` (config
  schema ~532), `server/src/sim/combat.rs` (damage is point-distance vs
  `splash_radius`, ~160–161).

**Problem.** `hit_radius` is exported in `welcome.ship_specs` and editable via the
config API, but nothing in `sim/` reads it. Bot authors will build strategy around a
hull extent that doesn't exist; operators tuning it change nothing.

**Fix (choose one, with maintainer sign-off).** (a) Implement it: hit test becomes
`distance <= splash_radius + hit_radius` — a balance change invalidating old replays;
or (b) remove it from `ship_specs` and the config schema — a breaking protocol change
(4-place sync + version bump). Do not leave it dead.

**Verify.** (a): combat unit tests for edge distances; (b): example bots still run;
protocol docs/SDK/spectator updated together.

---

## Phase 6 — SDK & examples polish

### F-21 — powerful_bot's velocity filter converges to half the true velocity

- **Severity:** Medium (it's the flagship example). **Effort:** S.
- **Files:** `examples/powerful_bot.py` (dead-reckon step ~264–272, velocity update
  ~305–311).

**Problem.** The velocity delta is measured against a position already dead-reckoned
forward by `vel*dt` this tick, so the measured `vx` is the innovation
(`v_true − v_est`), and the blend `0.6*vx + 0.4*prev.vel` has fixed point
`0.5 * v_true`. Verified numerically: a target at 8.0 u/s converges to an estimate of
exactly 4.0. Every led shot under-leads by 50%.

**Fix.** Keep the last *observed* position separately and compute
`vx = (obs - prev_obs) / dt_between_observations`, then blend; or blend as a proper
alpha filter on the innovation (`vel += alpha * innovation`). Add a short comment on
the chosen filter.

**Verify.** Quick self-test (script or docstring test): 200 noiseless ticks tracking
a constant-velocity target → estimate within 5% of truth.

### F-22 — `Welcome` not exported; README StatBot example crashes on import

- **Severity:** Low. **Effort:** XS.
- **Files:** `sdk-python/naval_sdk/__init__.py` (import list + `__all__`),
  `sdk-python/README.md` (~992).

**Problem.** README's StatBot does `from naval_sdk import ... Welcome ...` →
`ImportError`. Every example bot works around it via `naval_sdk.protocol`.

**Fix.** Export `Welcome` (and audit siblings like `GameOver`, `ShipSpecs` for
consistency) from `__init__.py`; keep the README import as-is once it works.

**Verify.** `python -c "from naval_sdk import Welcome"`; run the README StatBot
snippet against a local server.

### F-23 — SDK `run()` raises instead of returning when the server closes mid-send

- **Severity:** Low. **Effort:** S.
- **Files:** `sdk-python/naval_sdk/bot.py` (sends at ~199–208, ~239, ~265).

**Problem.** Only `ws.recv()` is wrapped in `except ConnectionClosed`. If the server
closes between delivering a tick and the SDK sending the command (abort, kick),
`ConnectionClosed` propagates out of `run_async`, so `run()` raises rather than
returning as documented.

**Fix.** Wrap the outbound sends (or the loop body) so `ConnectionClosed` on send
exits the loop cleanly, matching the recv path.

**Verify.** Test with a fake server that closes immediately after sending a tick →
`run()` returns normally.

### F-24 — "Up to 2" vs "exactly 2" powerup selection docs conflict

- **Severity:** Low. **Effort:** XS.
- **Files:** `server/src/protocol.rs` (doc comments ~39, ~163),
  `sdk-python/naval_sdk/bot.py` (`choose_powerups` docstring ~51). Authoritative
  behavior (don't change): `server/src/room.rs` ~1721–1727 rejects anything but
  exactly 2; `docs/PROTOCOL.md` already says "exactly two".

**Problem.** Two doc comments and the SDK docstring say "up to 2"; a bot sending one
pick gets `powerup_wrong_count` and silently plays vanilla.

**Fix.** Align all wording to "exactly 2 distinct". Consider having the SDK
log a loud warning if `choose_powerups` returns a length ≠ 0 and ≠ 2 before sending.

**Verify.** Grep for "up to 2" across the repo → zero hits.

---

## Phase 7 — Docs, types & hygiene (safe, independent, any order)

### F-25 — PROTOCOL.md documents the wrong Monte Carlo per-match seed formula

- **Files:** `docs/PROTOCOL.md` (~535), truth: `server/src/monte_carlo.rs` (~275–280).
- **Fix.** Document the actual derivation:
  `splitmix64_finalize(mc_seed ^ (match_index * 0x9E37_79B9_7F4A_7C15))`. Include the
  finalizer constants or reference the function by name so offline reproduction works.
- **Verify.** Compute one seed by hand from the doc and compare against a
  `monte_carlo.rs` unit test value.

### F-26 — Spectator TS types missing server-serialized fields

- **Files:** `spectator/src/types/protocol.ts` (`BotTickEvent` ~158–160,
  `ReplayBotInfo` ~163–167), truth: `server/src/protocol.rs` (~203–218),
  `server/src/replay.rs` (~86–106, ~168–172); also `docs/PROTOCOL.md` §2.6.2 example
  (missing `selected_powerups`).
- **Fix.** Add the `powerup_activated` variant to `BotTickEvent`; add
  `selected_powerups?`, `spawn_pos`, `spawn_heading_deg` to `ReplayBotInfo`; complete
  the PROTOCOL.md example. Coordinate with F-08 (which changes the event shape) —
  if F-08 is done first, mirror its final shape.
- **Verify.** `npm run build` type-checks; perspective viewer renders a replay
  containing a powerup activation.

### F-27 — system-design.md wire examples drifted

- **Files:** `system-design.md` (welcome example ~227–234, spectator world example
  ~292–296), truth: `server/src/protocol.rs` (shells use `id_index`, ~290–296).
- **Fix.** Update the examples: add `available_powerups` to `welcome`; shells use
  `id_index`; ship entries include the current field set. Or replace the inline
  examples with pointers to `docs/PROTOCOL.md` (already the deferral pattern used at
  the top of the doc).

### F-28 — SDK README documents the wrong default map size

- **Files:** `sdk-python/README.md` (~192: "1000 × 1000"), truth:
  `server/src/config.rs` (~20: default `700x700`).
- **Fix.** Correct to 700×700 and add "operator-configurable via `--map`; read the
  actual bounds from `welcome`". Bots hardcoding 1000 steer into walls (2 HP per
  bump per tick).

### F-29 — Dead `connection_limit` error code

- **Files:** `server/src/protocol.rs` (~326), context: `server/src/net.rs` (~700–707
  rejects with HTTP 503 pre-upgrade).
- **Fix.** Delete the constant (docs already correctly omit it), or emit it
  post-upgrade — deleting is simpler and matches documented behavior.

### F-30 — `tokio-tungstenite` is a production dependency used only by tests

- **Files:** `server/Cargo.toml` (~9).
- **Fix.** Move to a new `[dev-dependencies]` section. `cargo build` and
  `cargo test` must both stay green.

### F-31 — Violation-disconnect test passes if the server crashes

- **Files:** `server/tests/protocol_validation.rs` (~104–107 counts `Err(_)`/`None`
  as success; harness comment ~29–31 is wrong — the receiver is dropped, closing the
  room channel).
- **Fix.** Assert the `too_many_violations` error frame is received and the close is
  a proper WS close (policy code), not a transport error. Fix the harness: keep the
  receiver alive (or spawn a drain task) and correct the comment so future tests that
  complete a handshake don't mysteriously fail.

### F-32 — Admin broadcast channel is never wired in production

- **Files:** `server/src/room.rs` (`set_admin_broadcast` used only by tests, doc
  comment ~455–457 claims main.rs wires it), `server/src/main.rs`.
- **Fix.** Either wire it (only if an admin WS push feature is actually planned) or
  delete the dead plumbing and fix the comment. Prefer deletion — hackathon scope.

### F-33 — Monte Carlo runs permanently mutate the room's seed and SimConfig

- **Files:** `server/src/room.rs` (`self.seed = seed` in `mc_begin_next_match` ~2186,
  config override in `start_monte_carlo` ~2082–2085, `transition_to_lobby`).
- **Fix.** Save the operator's seed and `SimConfig` when an MC run starts; restore
  both when the run ends/aborts (natural end, `mc_abort`, and the F-03 abort path).
- **Verify.** Unit test: run an MC batch, return to lobby, start a normal match →
  replay header records the operator's original seed and config.

### F-34 — Spectator fallback constants drifted from server defaults

- **Files:** `spectator/src/lib/constants.ts` (~13–14: `MAX_AMMO = 20`,
  `MAX_FORWARD_SPEED = 6.0`), truth: `server/src/sim/constants.rs` (ammo 250, max
  forward speed 9.0).
- **Fix.** Sync the values and extend the file-header comment: these must equal the
  server defaults in `sim/constants.rs` and both files change together (guardrail
  also added to CLAUDE.md). Rebuild `dist/`, commit.
