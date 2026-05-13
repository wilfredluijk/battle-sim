# naval-sdk (Java)

Reference JVM SDK for the **battle-sim** naval hackathon game. Subclass
`Bot`, override `onTick`, and the SDK owns the WebSocket transport,
handshake, JSON framing, and message dispatch. You write strategy, not
plumbing.

```java
import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.WorldView;

public final class ForwardBot extends Bot {
    @Override
    public Command onTick(WorldView view) {
        return new Command().throttle(1.0f).sensorMode(SensorMode.ACTIVE);
    }

    public static void main(String[] args) {
        BotRunner.run(new ForwardBot(), "localhost", 7878, "forward");
    }
}
```

That bot connects, completes the handshake, and drives its ship straight
ahead until the match ends.

> **Read first:** [`../docs/SDK_GUIDE.md`](../docs/SDK_GUIDE.md) — the
> shared, language-agnostic guide covering the match lifecycle,
> coordinate system, common pitfalls, and protocol versioning. This
> README focuses on the Java-specific surface.

---

## Table of contents

1. [Requirements](#requirements)
2. [Install](#install)
3. [Quickstart](#quickstart)
4. [API reference](#api-reference)
5. [Example bots](#example-bots)
6. [Logging and debugging](#logging-and-debugging)
7. [Escape hatches: raw frames](#escape-hatches-raw-frames)
8. [Testing your bot](#testing-your-bot)

---

## Requirements

- JDK 17 or newer (records, sealed interfaces, switch expressions).
- Maven 3.6+. Gradle works too — see [Gradle setup](#using-with-gradle).

Runtime dependencies, pulled by Maven:

| dependency        | version | why                                       |
|-------------------|---------|-------------------------------------------|
| Java-WebSocket    | 1.5.7   | WebSocket client (blocking, lightweight). |
| Jackson Databind  | 2.17.2  | JSON parse/print of every frame.          |
| JUnit Jupiter     | 5.10.3  | Test scope only.                          |

---

## Install

### From source (local Maven repo)

```bash
cd sdk-java
mvn install                  # publishes naval-sdk:0.1.0 to ~/.m2
```

Then depend on it from your bot's `pom.xml`:

```xml
<dependency>
    <groupId>com.battlesim</groupId>
    <artifactId>naval-sdk</artifactId>
    <version>0.1.0</version>
</dependency>
```

### Using with Gradle

```kotlin
repositories { mavenLocal(); mavenCentral() }
dependencies {
    implementation("com.battlesim:naval-sdk:0.1.0")
}
```

### Build & test the SDK itself

```bash
mvn test                     # JUnit suite (16 tests)
mvn package                  # produces target/naval-sdk-0.1.0.jar
```

---

## Quickstart

```bash
# 1. start the server
cd server
cargo run -- --port 7878 --tick-hz 10 --seed 42

# 2. build and run your bot
cd ../my-bot
mvn -q compile exec:java -Dexec.mainClass=com.example.ForwardBot

# 3. in the server terminal, start the match
room start main
```

If you just want to ship a competitive bot fast, jump straight to the
**tactical toolkit** ([`docs/TACTICAL_TOOLKIT.md`](../docs/TACTICAL_TOOLKIT.md)) —
it provides opt-in `Tracker`/`Gunner`/`Helm`/`Evader` components and a
high-level `TacticalBot` you can subclass with a single `decide()` method.
The rest of this README documents the base, language-level API.

A self-contained `ForwardBot.java` you can save and run as-is:

```java
package com.example;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.WorldView;

public final class ForwardBot extends Bot {
    @Override
    public Command onTick(WorldView view) {
        return new Command()
                .throttle(1.0f)
                .rudder(0.0f)
                .sensorMode(SensorMode.ACTIVE);
    }

    public static void main(String[] args) {
        String host = args.length > 0 ? args[0] : "localhost";
        int port    = args.length > 1 ? Integer.parseInt(args[1]) : 7878;
        String name = args.length > 2 ? args[2] : "forward";
        BotRunner.run(new ForwardBot(), host, port, name);
    }
}
```

---

## API reference

All public types live under `com.battlesim.naval`. Protocol records are
under `com.battlesim.naval.protocol`.

### `Bot` (abstract)

```java
public abstract class Bot {
    // Populated by the runtime
    public Welcome welcome();
    public long lastTick();

    // Callbacks — override any you care about
    public void onWelcome(Welcome welcome) {}
    public void onGameStart(GameStart gameStart) {}
    public abstract Command onTick(WorldView view);
    public void onGameOver(GameOver result) {}
    public void onError(String code, String message) { /* logs */ }

    // Escape hatch
    public final void rawSend(ObjectNode payload);
}
```

**`onTick(view)` is the only callback you must implement.** Return a
`Command`. Return `null` or throw and the SDK falls back to a
hold-station command — your bot stays alive, exception is logged.

### `Command` (builder-style, mutable)

```java
new Command()
    .throttle(0.8f)
    .rudder(-0.3f)
    .sensorMode(SensorMode.PASSIVE)
    .fire(new FireCommand(47.5f, 300.0f));   // raw
```

| method                                | meaning                                            |
|---------------------------------------|----------------------------------------------------|
| `throttle(float)`                     | `-1 = full reverse, +1 = full ahead.`              |
| `rudder(float)`                       | `-1 = hard port, +1 = hard starboard.`             |
| `sensorMode(SensorMode)`              | `ACTIVE` (range, you're visible) or `PASSIVE`.     |
| `fire(FireCommand)`                   | Raw fire-control: bearing + range.                 |
| `fireAt(shooter, target)`             | Aim at a stationary target.                        |
| `fireAt(shooter, target, vel, speed)` | Lead a moving target using the lead solver.        |

Fire helpers compute bearing from `shooter` to the (optionally led)
target and set `range` to the distance, clamped server-side to
`maxShellRange`. Pass `view.self().pos()` as `shooter`.

### `WorldView`

What `onTick` receives. Immutable record.

| accessor       | type                | notes                                                         |
|----------------|---------------------|---------------------------------------------------------------|
| `tick()`       | `long`              | Monotonic tick number.                                        |
| `deadlineMs()` | `long`              | How long the server will wait for your `Command`.             |
| `self()`       | `SelfState`         | Your ship.                                                    |
| `contacts()`   | `List<Contact>`     | Filtered by your current sensor mode.                         |
| `events()`     | `List<TickEvent>`   | Things you can perceive (own hits, splashes in sensor range). |

Convenience: `view.nearestContact()` returns `Optional<Contact>` —
nearest contact with a known range.

### `Contact`

```java
record Contact(String id, ContactKind kind, Vec2 pos,
               float bearingDeg, OptionalDouble range, float confidence)
```

`id` is **per-tick** — not a stable ship ID. Data association across
ticks is your job. Passive contacts return `range = OptionalDouble.empty()`.

### `SelfState`

`pos`, `headingDeg`, `speed`, `hp`, `ammo`, `rudder`, `throttle`.

### `ShipSpecs`

Static gameplay constants from `welcome`. Persist on
`bot.welcome().shipSpecs()`. Most useful fields:
`shellSpeed()` (50.0), `maxShellRange()` (300.0),
`gunCooldownTicks()` (15), `hullHp()` (100), `maxAmmo()` (20).

### `GameOver`

```java
record GameOver(Optional<String> winner, long finalTick, String replayId)
```

`winner.isEmpty()` means a draw.

### `TickEvent` (sealed)

Pattern-match it:

```java
for (TickEvent ev : view.events()) {
    switch (ev) {
        case TickEvent.Hit h          -> log("took " + h.amount() + " dmg");
        case TickEvent.ShellSplash s  -> log("splash at " + s.pos());
        case TickEvent.Unknown u      -> {/* forward-compatible no-op */}
    }
}
```

### `Geometry` (math helpers)

All angles in degrees. Compass bearings: `0° = north (-y), 90° = east (+x)`.

```java
float distance(Vec2 a, Vec2 b);
float bearingTo(Vec2 from, Vec2 to);                  // in [0, 360)
Optional<Vec2> leadTarget(Vec2 shooter, Vec2 target,
                          Vec2 targetVel, float shellSpeed);
```

`leadTarget` returns `Optional.empty()` when no real intercept exists
(e.g. target outruns the shell).

### `BotRunner`

```java
Optional<GameOver> BotRunner.run(Bot bot, String host, int port, String name);
Optional<GameOver> BotRunner.run(Bot bot, String host, int port,
                                 String name, String version, String path);
```

Blocks until `game_over` or the WebSocket closes. Returns the
`GameOver` payload if the match ended cleanly, else `Optional.empty()`.

---

## Example bots

### Drift in a circle, fire blind

```java
import com.battlesim.naval.*;
import com.battlesim.naval.protocol.*;

public final class CircleBot extends Bot {
    @Override
    public Command onTick(WorldView view) {
        Command cmd = new Command()
                .throttle(0.6f)
                .rudder(0.4f)
                .sensorMode(SensorMode.ACTIVE);

        if (view.tick() % 30 == 0 && view.self().ammo() > 0) {
            float bearing = (view.tick() * 11) % 360;
            cmd.fire(new FireCommand(bearing, 250.0f));
        }
        return cmd;
    }

    public static void main(String[] args) {
        BotRunner.run(new CircleBot(), "localhost", 7878, "circle");
    }
}
```

### Chaser: active radar, pursue the nearest contact

```java
import com.battlesim.naval.*;
import com.battlesim.naval.protocol.*;
import java.util.Optional;

public final class ChaserBot extends Bot {
    @Override
    public Command onTick(WorldView view) {
        Optional<Contact> target = view.nearestContact();
        if (target.isEmpty()) {
            return new Command()
                    .throttle(0.5f).rudder(0.0f).sensorMode(SensorMode.ACTIVE);
        }

        Contact c = target.get();
        float myHeading = view.self().headingDeg();
        float want = Geometry.bearingTo(view.self().pos(), c.pos());
        // Signed turn in [-180, 180]
        float delta = ((want - myHeading + 540f) % 360f) - 180f;
        float rudder = Math.max(-1f, Math.min(1f, delta / 30f));

        Command cmd = new Command()
                .throttle(1.0f).rudder(rudder).sensorMode(SensorMode.ACTIVE);

        if (Math.abs(delta) < 5f
                && c.range().isPresent() && c.range().getAsDouble() < 280) {
            cmd.fireAt(view.self().pos(), c.pos());
        }
        return cmd;
    }

    public static void main(String[] args) {
        BotRunner.run(new ChaserBot(), "localhost", 7878, "chaser");
    }
}
```

### Sniper: passive listen, ping only to commit a shot

```java
import com.battlesim.naval.*;
import com.battlesim.naval.protocol.*;
import java.util.Optional;

public final class SniperBot extends Bot {
    private int pingFor = 0;

    @Override
    public Command onTick(WorldView view) {
        Optional<Contact> contact = view.contacts().isEmpty()
                ? Optional.empty()
                : Optional.of(view.contacts().get(0));

        SensorMode mode = SensorMode.PASSIVE;
        if (pingFor > 0) {
            pingFor--;
            mode = SensorMode.ACTIVE;
        }

        // Heard something on passive? Light up briefly for a range fix.
        if (contact.isPresent() && contact.get().range().isEmpty() && pingFor == 0) {
            pingFor = 3;
            mode = SensorMode.ACTIVE;
        }

        Command cmd = new Command().throttle(0.4f).sensorMode(mode);

        if (contact.isPresent()
                && contact.get().range().isPresent()
                && view.self().ammo() > 0) {
            cmd.fireAt(view.self().pos(), contact.get().pos());
        }
        return cmd;
    }

    public static void main(String[] args) {
        BotRunner.run(new SniperBot(), "localhost", 7878, "sniper");
    }
}
```

### Lifecycle hooks: track per-match stats

```java
import com.battlesim.naval.*;
import com.battlesim.naval.protocol.*;

public final class StatBot extends Bot {
    @Override
    public void onWelcome(Welcome w) {
        System.out.printf(
            "I am %s, ship %s%n"
          + "Shells fly at %.1f units/s, max range %.1f%n",
            w.botId(), w.shipId(),
            w.shipSpecs().shellSpeed(), w.shipSpecs().maxShellRange());
    }

    @Override
    public void onGameStart(GameStart gs) {
        System.out.printf("Match started at tick %d, heading %.1f°%n",
                gs.tick(), gs.startingHeadingDeg());
    }

    @Override
    public Command onTick(WorldView view) {
        return new Command().throttle(0.5f).sensorMode(SensorMode.PASSIVE);
    }

    @Override
    public void onGameOver(GameOver r) {
        if (r.winner().map(w -> w.equals(welcome().botId())).orElse(false)) {
            System.out.println("Victory.");
        } else if (r.winner().isEmpty()) {
            System.out.println("Draw at tick " + r.finalTick());
        } else {
            System.out.println("Defeated by " + r.winner().get()
                             + ". Replay: " + r.replayId());
        }
    }
}
```

---

## Logging and debugging

The SDK uses `java.util.logging`. Logger names are
`com.battlesim.naval.Bot` and `com.battlesim.naval.BotRunner`. To see
everything during development:

```java
import java.util.logging.*;

Logger.getLogger("com.battlesim.naval").setLevel(Level.FINE);
for (Handler h : Logger.getLogger("").getHandlers()) h.setLevel(Level.FINE);
```

Or with a `logging.properties`:

```properties
.level = INFO
com.battlesim.naval.level = FINE
```

Useful patterns:

- Print `view.deadlineMs()` once after `onWelcome` to know your tick
  budget on the *current* server config.
- React to `Hit` events so you know when the enemy is finding you (see
  the sealed pattern-match example above).
- `welcome().shipSpecs()` carries every gameplay constant. Read them
  from there rather than hard-coding numbers — that way your bot keeps
  working if balance changes.

---

## Escape hatches: raw frames

If the typed API doesn't fit (debugging, prototyping a new protocol
field, building an inspector), bypass it with `rawSend`:

```java
import com.fasterxml.jackson.databind.node.JsonNodeFactory;
import com.fasterxml.jackson.databind.node.ObjectNode;

@Override
public Command onTick(WorldView view) {
    ObjectNode raw = JsonNodeFactory.instance.objectNode();
    raw.put("type", "command");
    raw.put("tick", view.tick());
    raw.put("throttle", 0.0);
    raw.put("rudder", 0.0);
    raw.put("sensor_mode", "active");
    rawSend(raw);
    return new Command();   // SDK still emits a normal command after this — be aware
}
```

There is no `rawRecv` — the runtime owns the inbound stream and fans it
out to your typed callbacks. If you need raw inbound JSON, override
`onTick` and re-derive what you need from `WorldView`, or write a
debugging spectator client.

---

## Testing your bot

You don't need a running server to unit-test logic. Build a `WorldView`
from a JSON literal and call `onTick` directly:

```java
import com.battlesim.naval.protocol.WorldView;
import com.fasterxml.jackson.databind.ObjectMapper;

ObjectMapper mapper = new ObjectMapper();
String frame = """
    { "type": "tick", "tick": 100, "deadline_ms": 80,
      "self": {"pos":[200,500],"heading_deg":90,"speed":4.1,
               "hp":100,"ammo":14,"rudder":0,"throttle":0.8},
      "contacts": [{"id":"c1","kind":"ship","pos":[450,510],
                    "bearing_deg":88,"range":247,"confidence":0.85}],
      "events": [] }
    """;
WorldView view = WorldView.from(mapper.readTree(frame));
Command cmd = new MyBot().onTick(view);
assertTrue(cmd.fireValue().isPresent());
```

If your bot uses `Random`, seed it yourself so matches are reproducible.

---

## Java-specific pitfalls

The shared
[common pitfalls](../docs/SDK_GUIDE.md#common-pitfalls) list applies to
every SDK. One extra thing to watch out for on the JVM:

- **`Bot` is not thread-safe** — the runtime calls your callbacks on a
  single thread. If you spawn workers yourself, synchronize access to
  bot state.

---

## See also

- [`../docs/SDK_GUIDE.md`](../docs/SDK_GUIDE.md) — match lifecycle,
  coordinate system, common pitfalls, versioning policy.
- [`../docs/PROTOCOL.md`](../docs/PROTOCOL.md) — full wire protocol
  spec.
- [`../sdk-python/README.md`](../sdk-python/README.md) — the Python
  equivalent.

The SDK artifact version is set in `pom.xml`; the wire protocol version
comes from the server in the `welcome` frame. `TickEvent.Unknown` is
how the Java SDK stays forward-compatible with additive protocol
changes — see
[`SDK_GUIDE.md`](../docs/SDK_GUIDE.md#versioning-and-compatibility)
for the full compatibility policy.
