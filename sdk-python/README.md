# naval-sdk

Python SDK for writing bots against the battle-sim naval server.

## Install

```bash
pip install -e .
```

## Minimal bot

```python
from naval_sdk import Bot, Command, WorldView, run


class Forward(Bot):
    def on_tick(self, view: WorldView) -> Command:
        return Command(throttle=1.0, rudder=0.0, sensor_mode="active")


if __name__ == "__main__":
    run(Forward(), host="localhost", port=7878, name="forward")
```

See `examples/` in the repo root for richer bots.
