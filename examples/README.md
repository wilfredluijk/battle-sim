# Bot examples

Language-specific bot examples live in subfolders:

- [`python`](python) — Python bots:
  - `circle_bot.py`, `chaser_bot.py`, `sniper_bot.py` — bare protocol (Layer 0).
  - `tracking_bot.py`, `tactician_bot.py` — composed L2 tactical components
    (`Tracker`, `Gunner`, `Helm`, `Evader`, `SensorPolicy`).
  - `strategist_bot.py` — Layer 3 `TacticalBot` with a single `decide()` method.
  - See [`../docs/TACTICAL_TOOLKIT.md`](../docs/TACTICAL_TOOLKIT.md) for the
    layered SDK overview.
- [`java`](java) — Java bots built on `sdk-java`.
