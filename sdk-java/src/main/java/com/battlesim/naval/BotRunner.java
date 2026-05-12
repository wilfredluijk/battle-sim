package com.battlesim.naval;

import com.battlesim.naval.protocol.GameOver;
import com.battlesim.naval.protocol.GameStart;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.JsonNodeFactory;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.net.URI;
import java.util.Optional;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.logging.Level;
import java.util.logging.Logger;
import org.java_websocket.client.WebSocketClient;
import org.java_websocket.handshake.ServerHandshake;

/**
 * Connects a {@link Bot} to a battle-sim server and pumps messages until {@code game_over}.
 *
 * <p>Trust model: the server is authoritative. Malformed server messages are logged and
 * dropped — the SDK never throws on bad input from the server. Bot callbacks may throw
 * without taking down the connection.
 */
public final class BotRunner {

    private static final Logger LOG = Logger.getLogger(BotRunner.class.getName());
    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final String DEFAULT_VERSION = "naval-sdk-java/0.1.0";

    private BotRunner() {}

    public static Optional<GameOver> run(Bot bot, String host, int port, String name) {
        return run(bot, host, port, name, DEFAULT_VERSION, "/bot");
    }

    public static Optional<GameOver> run(
            Bot bot, String host, int port, String name, String version, String path) {
        URI uri = URI.create("ws://" + host + ":" + port + path);
        LOG.info("connecting to " + uri + " as " + name);

        BlockingQueue<JsonNode> typedQueue = new LinkedBlockingQueue<>();
        CountDownLatch closeLatch = new CountDownLatch(1);

        InternalClient client = new InternalClient(uri, typedQueue, closeLatch);

        Connection conn = new Connection(client);
        bot.connection = conn;

        try {
            try {
                client.connectBlocking();
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return Optional.empty();
            }

            sendJson(client, helloFrame(name, version));

            boolean readySent = false;
            GameOver result = null;

            while (true) {
                JsonNode msg;
                try {
                    msg = typedQueue.poll(1, TimeUnit.SECONDS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    break;
                }
                if (msg == null) {
                    if (closeLatch.getCount() == 0) break;
                    continue;
                }

                JsonNode typeNode = msg.get("type");
                if (typeNode == null) {
                    LOG.warning("ignoring frame with no type: " + msg);
                    continue;
                }
                String type = typeNode.asText();

                switch (type) {
                    case "welcome" -> {
                        Welcome welcome;
                        try {
                            welcome = Welcome.from(msg);
                        } catch (Exception ex) {
                            LOG.log(Level.WARNING, "malformed welcome: " + msg, ex);
                            continue;
                        }
                        bot.setWelcome(welcome);
                        safe(() -> bot.onWelcome(welcome));
                        if (!readySent) {
                            sendJson(client, readyFrame());
                            readySent = true;
                        }
                    }
                    case "game_start" -> {
                        try {
                            GameStart gs = GameStart.from(msg);
                            safe(() -> bot.onGameStart(gs));
                        } catch (Exception ex) {
                            LOG.log(Level.WARNING, "malformed game_start: " + msg, ex);
                        }
                    }
                    case "tick" -> {
                        WorldView view;
                        try {
                            view = WorldView.from(msg);
                        } catch (Exception ex) {
                            LOG.log(Level.WARNING, "malformed tick: " + msg, ex);
                            continue;
                        }
                        bot.setLastTick(view.tick());
                        Command cmd;
                        try {
                            cmd = bot.onTick(view);
                            if (cmd == null) cmd = new Command();
                        } catch (Exception ex) {
                            LOG.log(Level.WARNING, "onTick raised; sending hold-station", ex);
                            cmd = new Command();
                        }
                        sendJson(client, cmd.toJson(view.tick()));
                    }
                    case "game_over" -> {
                        try {
                            result = GameOver.from(msg);
                            final GameOver finalResult = result;
                            safe(() -> bot.onGameOver(finalResult));
                        } catch (Exception ex) {
                            LOG.log(Level.WARNING, "malformed game_over: " + msg, ex);
                        }
                    }
                    case "error" -> {
                        String code = textOr(msg.get("code"), "unknown");
                        String message = textOr(msg.get("message"), "");
                        safe(() -> bot.onError(code, message));
                    }
                    default -> LOG.fine("ignoring unknown message type " + type);
                }

                if ("game_over".equals(type)) break;
            }

            return Optional.ofNullable(result);
        } finally {
            bot.connection = null;
            client.close();
        }
    }

    private static void sendJson(WebSocketClient client, JsonNode node) {
        try {
            client.send(MAPPER.writeValueAsString(node));
        } catch (Exception e) {
            LOG.log(Level.WARNING, "failed to send frame", e);
        }
    }

    private static String textOr(JsonNode n, String fallback) {
        return (n == null || n.isNull()) ? fallback : n.asText();
    }

    private static ObjectNode helloFrame(String name, String version) {
        ObjectNode n = JsonNodeFactory.instance.objectNode();
        n.put("type", "hello");
        n.put("name", name);
        n.put("version", version);
        return n;
    }

    private static ObjectNode readyFrame() {
        ObjectNode n = JsonNodeFactory.instance.objectNode();
        n.put("type", "ready");
        return n;
    }

    private static void safe(Runnable r) {
        try {
            r.run();
        } catch (Exception ex) {
            LOG.log(Level.WARNING, "bot callback raised", ex);
        }
    }

    /** Visible to {@link Bot#rawSend}. */
    static final class Connection {
        private final InternalClient client;

        Connection(InternalClient client) {
            this.client = client;
        }

        void send(ObjectNode payload) {
            sendJson(client, payload);
        }
    }

    private static final class InternalClient extends WebSocketClient {
        private final BlockingQueue<JsonNode> typedQueue;
        private final CountDownLatch closeLatch;

        InternalClient(URI uri, BlockingQueue<JsonNode> typedQueue, CountDownLatch closeLatch) {
            super(uri);
            this.typedQueue = typedQueue;
            this.closeLatch = closeLatch;
        }

        @Override
        public void onOpen(ServerHandshake handshake) {
            LOG.fine("WebSocket open");
        }

        @Override
        public void onMessage(String message) {
            try {
                typedQueue.put(MAPPER.readTree(message));
            } catch (Exception ex) {
                LOG.log(Level.WARNING, "ignoring non-JSON or interrupted frame: " + message, ex);
            }
        }

        @Override
        public void onClose(int code, String reason, boolean remote) {
            LOG.info("WebSocket closed code=" + code + " reason=" + reason);
            closeLatch.countDown();
        }

        @Override
        public void onError(Exception ex) {
            LOG.log(Level.WARNING, "WebSocket error", ex);
        }
    }
}
