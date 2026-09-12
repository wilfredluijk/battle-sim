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
- `loadout_bot.py` — picks two powerups and chains them in a burst combo.
  See [`../docs/POWERUPS.md`](../docs/POWERUPS.md) for the full catalog.

See [`../sdk-python/README.md`](../sdk-python/README.md) for the layered
SDK overview, the base API, and the tactical toolkit reference.

## Connect to the training server

From the repository root, install the current SDK and choose a bot:

```bash
python -m pip install -e ./sdk-python
python examples/circle_bot.py --env-file .deployment-secrets/player01.env
```

Participants use their own supplied `playerNN.env` file, wherever it is saved.
All six examples accept the same connection options. They connect directly to
**`wss://93.190.187.250/bot`**; no administrator tunnel is needed for bots.
The server assigns the identity associated with the participant credential,
such as `player01`, regardless of the requested `--name`.

The file must contain both settings:

```dotenv
BATTLE_SERVER_URL=wss://93.190.187.250/bot
BATTLE_BOT_TOKEN=your-participant-credential
```

Blank lines, comments, quoted values and optional `export` prefixes are supported.
Files are parsed as data: shell commands and variable references are not expanded.
File settings replace inherited connection settings together, so selecting a file
cannot reuse another team's token from the environment. Keep each participant
file private; use a distinct credential for every simultaneously connected bot.
The administrator password is for the browser login only.

Alternatively, export `BATTLE_SERVER_URL` and `BATTLE_BOT_TOKEN` before launching
an example. Without a URL setting, the examples default to the public training
server. A missing credential on a `wss://` connection produces a local error with
setup instructions. TLS certificate verification remains enabled.

| Option | Behavior |
|---|---|
| `--env-file PATH` | Load one participant's server URL and token. |
| `--url wss://HOST/bot` | Override the server URL while keeping the selected credential. |
| `--host HOST --port PORT` | Explicit `ws://` connection, overriding file/environment URLs; defaults to `localhost` and `7878` for omitted parts. |
| `--name NAME` | Requested display name; authenticated identity is assigned by the server. |

Use `--url` or `--host`/`--port`, not both. The examples' bot-specific options,
such as the circle bot's `--seed` and tactician's `--verbose`, are unchanged.

## Local development

To connect to a local server explicitly:

```bash
python examples/circle_bot.py --host localhost --port 7878 --name circler
```

A local server must have participant credentials configured, or be started with
`--allow-unauthenticated-bots` for local development. The bot joins the lobby and
acknowledges the active rules. Start the match from the administrator UI.

## Run all examples with Docker

For the training server, the operator can run six bots with the six separate
participant files `player01.env` through `player06.env`:

```bash
docker compose -f docker-compose.bots.yml -f docker-compose.bots.remote.yml up --build
```

This reads the files from `.deployment-secrets/`; set `BATTLE_CREDENTIALS_DIR` to
use another directory. Files are supplied at container startup and are excluded
from the bot image. Each file's `BATTLE_SERVER_URL` selects the server.

| Example | Participant file |
|---|---|
| Circle | `player01.env` |
| Powerful | `player02.env` |
| Strategist | `player03.env` |
| Tactician | `player04.env` |
| Tracking | `player05.env` |
| Loadout | `player06.env` |

The original `docker compose -f docker-compose.bots.yml up --build` command
continues to use the local Docker host via `SERVER_HOST` / `SERVER_PORT`.
Stop the bots with Ctrl+C, or run `down` with the same compose-file arguments.
