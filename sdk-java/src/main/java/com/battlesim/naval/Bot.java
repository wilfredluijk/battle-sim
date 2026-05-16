package com.battlesim.naval;

import com.battlesim.naval.protocol.GameOver;
import com.battlesim.naval.protocol.GameStart;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.logging.Logger;

/**
 * Base class for naval-battle bots. Subclass and override {@link #onTick(WorldView)};
 * everything else is optional.
 *
 * <p>Lifecycle (all callbacks run on the runner's single dispatch thread):
 * <ol>
 *   <li>{@link #onWelcome(Welcome)} — once, after the handshake.</li>
 *   <li>{@link #onGameStart(GameStart)} — once, when the room enters {@code running}.</li>
 *   <li>{@link #onTick(WorldView)} — every tick (default 10 Hz). <b>Override me.</b></li>
 *   <li>{@link #onGameOver(GameOver)} — once, just before the WebSocket closes.</li>
 *   <li>{@link #onError(String, String)} — any time the server sends an {@code error} frame.</li>
 * </ol>
 *
 * <p>All callbacks are best-effort. If one throws, the runtime logs it and continues —
 * bot crashes never tank the WebSocket connection. If {@link #onTick} throws or returns
 * {@code null}, the SDK emits a hold-station {@link Command}.
 *
 * <p>Threading: this class is <b>not thread-safe</b>. Callbacks are serialized by the
 * runtime. If you spawn your own worker threads, you must synchronize their access to
 * any bot state yourself.
 */
public abstract class Bot {
    private static final Logger LOG = Logger.getLogger(Bot.class.getName());

    private Welcome welcome;
    private long lastTick;
    /** Set by the runtime so {@link #rawSend(ObjectNode)} can reach the wire. */
    BotRunner.Connection connection;

    /** The {@code welcome} frame from the server, or {@code null} before the handshake. */
    public Welcome welcome() {
        return welcome;
    }

    /** Tick number of the most recent {@link #onTick} invocation. */
    public long lastTick() {
        return lastTick;
    }

    void setWelcome(Welcome w) {
        this.welcome = w;
    }

    void setLastTick(long t) {
        this.lastTick = t;
    }

    /**
     * Fires once, right after the {@code welcome} frame is parsed.
     *
     * <p>Use it to stash gameplay constants ({@code welcome.shipSpecs().shellSpeed()},
     * {@code maxShellRange()}, etc.) on {@code this} so {@link #onTick} can read them
     * cheaply. Runs before the SDK sends {@code ready} to the server.
     */
    public void onWelcome(Welcome welcome) {}

    /**
     * Fires when the operator transitions the room to {@code running}.
     *
     * <p>The starting position and heading are also reflected on the <em>next</em>
     * {@link #onTick}'s {@code view.self()}, so most bots can ignore this hook.
     */
    public void onGameStart(GameStart gameStart) {}

    /**
     * Decide what to do this tick. <b>Override me.</b>
     *
     * <p>Called every simulation tick (default: 10 Hz). Return a {@link Command} —
     * the SDK serializes it back to the server before {@code view.deadlineMs()}
     * elapses. If you return {@code null} or throw, the SDK logs and sends a
     * hold-station command, keeping the connection alive.
     *
     * <p>See the README's "Example bots" section for typical patterns.
     */
    public abstract Command onTick(WorldView view);

    /**
     * Fires when the server announces a match result.
     *
     * <p>{@code result.winner()} holds the winning {@code bot_id}, or is empty for a
     * draw / aborted match. The replay JSONL lives at {@code replays/<replayId>.jsonl}
     * on the server.
     *
     * <p>Return {@code false} to close the connection and exit the run loop. Return
     * {@code true} to stay connected and wait for the next match — the server emits a
     * {@code lobby} frame followed by another {@code game_start} when the operator
     * starts another match. The SDK auto-sends {@code ready} again when it sees the
     * {@code lobby} frame, so the default is "stay around for the next round".
     */
    public boolean onGameOver(GameOver result) {
        return true;
    }

    /**
     * Fires when the server returns the room to the lobby after a match.
     *
     * <p>{@code tick} is always 0 (the next match's starting tick). The SDK auto-sends
     * {@code ready} immediately after this callback, so most bots can ignore the hook.
     * Override to reset per-game state (kill counters, plans).
     */
    public void onLobby(long tick) {}

    /**
     * Fires whenever the server sends a typed {@code error} frame.
     *
     * <p>Common codes: {@code late_command}, {@code cooldown_active}, {@code no_ammo}.
     * Override to react (for instance, back off pings when you keep missing the
     * deadline). The default behaviour is to log at WARNING level.
     */
    public void onError(String code, String message) {
        LOG.warning("server error code=" + code + ": " + message);
    }

    /**
     * Send an arbitrary JSON object to the server, bypassing the typed API.
     *
     * <p>Useful for prototyping a new protocol field or for inspector-style tools. There is
     * intentionally no {@code rawRecv}: the runtime consumes every inbound frame and fans it
     * out to the typed callbacks. If you need raw inbound JSON, do it inside {@link #onTick}.
     *
     * @throws IllegalStateException if called before the connection is open.
     */
    public final void rawSend(ObjectNode payload) {
        if (connection == null) {
            throw new IllegalStateException("rawSend called before connection is open");
        }
        connection.send(payload);
    }
}
