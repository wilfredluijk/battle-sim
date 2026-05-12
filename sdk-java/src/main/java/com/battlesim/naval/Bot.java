package com.battlesim.naval;

import com.battlesim.naval.protocol.GameOver;
import com.battlesim.naval.protocol.GameStart;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.logging.Logger;

/**
 * Base class for naval-battle bots. Subclass and override {@link #onTick(WorldView)}.
 *
 * <p>All callbacks are best-effort. If a callback throws, the runtime logs and continues —
 * bot crashes never tank the WebSocket connection.
 */
public abstract class Bot {
    private static final Logger LOG = Logger.getLogger(Bot.class.getName());

    private Welcome welcome;
    private long lastTick;
    /** Set by the runtime so {@link #rawSend(ObjectNode)} can reach the wire. */
    BotRunner.Connection connection;

    public Welcome welcome() {
        return welcome;
    }

    public long lastTick() {
        return lastTick;
    }

    void setWelcome(Welcome w) {
        this.welcome = w;
    }

    void setLastTick(long t) {
        this.lastTick = t;
    }

    public void onWelcome(Welcome welcome) {}

    public void onGameStart(GameStart gameStart) {}

    public abstract Command onTick(WorldView view);

    public void onGameOver(GameOver result) {}

    public void onError(String code, String message) {
        LOG.warning("server error code=" + code + ": " + message);
    }

    /**
     * Send an arbitrary JSON object to the server, bypassing the typed API. Receive-side
     * escape hatch is intentionally omitted — the runtime consumes every inbound frame and
     * fans it out to the typed callbacks. Override {@link #onTick} (or the other callbacks)
     * with raw {@link JsonNode} access via the typed view's source frame if you need it.
     */
    public final void rawSend(ObjectNode payload) {
        if (connection == null) {
            throw new IllegalStateException("rawSend called before connection is open");
        }
        connection.send(payload);
    }
}
