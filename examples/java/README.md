# Java bot examples

This folder contains three runnable Java bots built on `sdk-java`:

- `SimpleCircleBot` drives in circles and fires random long-range shots.
- `TrackingCircleBot` still drives in circles, but uses active pings to build a motion track and passive bearings to keep that track alive between pings.
- `StrongTacticalBot` keeps multiple tracks, scores targets, manages radar exposure, reacts to hits and splashes, avoids walls, and fires led shots when the track quality is good enough.

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
```

Each bot accepts optional arguments:

```text
host port name
```

For example:

```bash
mvn -q exec:java -Dexec.mainClass=com.battlesim.examples.StrongTacticalBot -Dexec.args="localhost 7878 strong-1"
```
