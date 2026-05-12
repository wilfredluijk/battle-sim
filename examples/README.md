# Java example bots

Three reference bots that build on the `naval-sdk`, in roughly increasing
order of sophistication. Read them top-to-bottom to see the API in
progressively heavier use.

| bot              | what it shows                                          |
|------------------|--------------------------------------------------------|
| `CircleBot`      | Smallest viable bot. Drives in a circle, fires blind.  |
| `TrackingBot`    | Stitches per-tick contact pings into stable tracks, leads moving targets. Still drives in a circle. |
| `HunterBot`      | Full combatant: tracking, range management, passive/active sensor switching, evasion, edge avoidance, lead solver. |

## Build

The bots depend on the SDK artifact published to your local Maven repo.

```bash
# 1. Install the SDK locally (once)
cd sdk-java
mvn -q install

# 2. Compile the examples
cd ../examples
mvn -q compile
```

## Run

In one terminal:

```bash
cd server
cargo run -- --port 7878 --tick-hz 10 --seed 42
```

In one terminal per bot:

```bash
cd examples
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.CircleBot
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.TrackingBot
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.HunterBot
```

Each `main` accepts `host port name` as optional positional arguments, e.g.

```bash
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.HunterBot \
    -Dexec.args="localhost 7878 hunter-1"
```

Then in the server's stdin:

```
room start main
```

Open `http://localhost:7878/` in a browser to watch.
