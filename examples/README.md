# Bot examples

Runnable Python bots demonstrating each tactical layer of the SDK:

- `circle_bot.py` — bare protocol (Layer 0). Drives in a circle and fires
  at random bearings.
- `powerful_bot.py` — bare protocol with hand-rolled tracking and fire
  control (Layer 0/1).
- `tracking_bot.py`, `tactician_bot.py` — composed L2 tactical components
  (`Tracker`, `Gunner`, `Helm`, `Evader`, `SensorPolicy`).
- `strategist_bot.py` — Layer 3 `TacticalBot` with a single `decide()`
  method.

See [`../sdk-python/README.md`](../sdk-python/README.md) for the layered
SDK overview, the base API, and the tactical toolkit reference.

## Running a bot

```bash
pip install -e ../sdk-python      # one-time install of the SDK
python circle_bot.py --host localhost --port 7878 --name circler
```

With the server running on `localhost:7878`, the bot connects, joins the
lobby, and signals ready. Start the match from the spectator UI at
`http://localhost:7878/` (admin login required).
