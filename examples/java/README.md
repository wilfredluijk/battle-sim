# Java bot examples

This folder contains six runnable Java bots built on `sdk-java`:

- `SimpleCircleBot` drives in circles and fires random long-range shots. Bare protocol (Layer 0).
- `TrackingCircleBot` still drives in circles, but uses active pings to build a motion track and passive bearings to keep that track alive between pings. Built on the Layer-2 tactical toolkit (`Tracker`, `Gunner`, `SensorPolicy`).
- `StrongTacticalBot` keeps multiple tracks, scores targets, manages radar exposure, reacts to hits and splashes, avoids walls, and fires led shots when the track quality is good enough. Built on the Layer-2 tactical toolkit (`Tracker`, `Gunner`, `Helm`, `Evader`, `SensorPolicy`).
- `StrategistBot` is the Layer-3 showcase: subclasses `TacticalBot` and overrides a single `decide()` method — the toolkit wires up everything else.
- `AcousticShadowBot` is a sound-first ambusher: it stays passive, triangulates bearing-only contacts across its own movement, exploits noisy active-radar opponents, confirms shots with short radar bursts, and evades after incoming fire.
- `ApexDuelistBot` is a one-on-one L1/L2 hybrid built to counter stealth-first duelists: it maintains passive-born tracks, confirms profitable shots with short active bursts, and changes course after pinging or firing.

See [`../../docs/TACTICAL_TOOLKIT.md`](../../docs/TACTICAL_TOOLKIT.md) for the layered SDK overview.

## Build

Install the SDK into your local Maven repository first:

```bash
cd ../../sdk-java
mvn install
```

Then compile the examples:

```bash
cd ../examples/java
mvn compile
```

## Run

Start the server in another terminal, then run one bot per process:

```bash
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.SimpleCircleBot
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.TrackingCircleBot
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.StrongTacticalBot
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.StrategistBot
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.AcousticShadowBot
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.ApexDuelistBot
```

Each bot accepts optional arguments:

```text
host port name
```

For example:

```bash
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.AcousticShadowBot -Dexec.args="localhost 7878 acoustic-1"
```
