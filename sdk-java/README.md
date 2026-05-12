# naval-sdk (Java)

JVM SDK for writing bots against the battle-sim naval server.

## Requirements

- JDK 17+
- Maven 3.6+

## Build & test

```bash
mvn test         # run unit tests
mvn package      # build the jar
mvn install      # publish to local ~/.m2 for use from other Maven projects
```

## Minimal bot

```java
import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.WorldView;

public class ForwardBot extends Bot {
    @Override
    public Command onTick(WorldView view) {
        return new Command()
            .throttle(1.0f)
            .rudder(0.0f)
            .sensorMode(SensorMode.ACTIVE);
    }

    public static void main(String[] args) {
        BotRunner.run(new ForwardBot(), "localhost", 7878, "forward");
    }
}
```

See `docs/PROTOCOL.md` for the wire contract and `examples/` for richer bots
(Python today; equivalent Java examples are tracked under Phase 10).

## Package layout

- `com.battlesim.naval.Bot`         — abstract base class for your bot.
- `com.battlesim.naval.BotRunner`   — connects, handshakes, dispatches callbacks.
- `com.battlesim.naval.Command`     — outbound command builder.
- `com.battlesim.naval.Geometry`    — `bearingTo`, `distance`, `leadTarget`.
- `com.battlesim.naval.protocol.*`  — typed views of every server-sent message.
