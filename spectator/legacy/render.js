// Naval Battle spectator renderer.
//
// Pulls `world` frames from `/spectate` over WebSocket and draws them on a 2D canvas.
// The simulation uses a top-left origin with +x right / +y down (canvas convention),
// so positions map straight through after the world→canvas scale.
//
// Map dimensions match the server default (1000x1000); if the server is configured
// differently we'll learn the size from the first frame's positions, but for the MVP
// we assume the default.

(() => {
  const MAP_WIDTH = 1000;
  const MAP_HEIGHT = 1000;
  const SHIP_RADIUS = 12; // visual radius; sim hit_radius is 8
  const ACTIVE_RADAR_RANGE = 350;
  const SPLASH_DRAW_MS = 600;
  const MAX_EVENTS = 20;

  // Stable color per player by bot_name. Cycle through a small palette.
  const COLOR_PALETTE = [
    "#6cb1ff",
    "#ef9a4a",
    "#9f7df7",
    "#58d68d",
    "#f4d35e",
    "#ff6f9c",
    "#5fd1c5",
    "#c47bff",
  ];
  const colorByName = new Map();
  function colorFor(name) {
    if (!colorByName.has(name)) {
      colorByName.set(name, COLOR_PALETTE[colorByName.size % COLOR_PALETTE.length]);
    }
    return colorByName.get(name);
  }

  // Default ship specs — used to scale the HP/ammo meters when the spectator never sees
  // a `welcome` frame. These match `ShipSpecs::DEFAULT` server-side.
  const MAX_HP = 100;
  const MAX_AMMO = 20;
  const MAX_FORWARD_SPEED = 6.0;
  const MAX_REVERSE_SPEED = 2.0;

  const canvas = document.getElementById("board");
  const ctx = canvas.getContext("2d");
  const statusEl = document.getElementById("status");
  const tickEl = document.getElementById("tick");
  const botsEl = document.getElementById("bots");
  const eventsEl = document.getElementById("events");

  // Bots we've seen at least once, keyed by ship id. We keep the last-known snapshot so
  // a disconnect (bot vanishes from the world frame) can still surface the final state
  // in the sidebar instead of silently dropping the card.
  const botCards = new Map();

  // Most recent world snapshot from the server. Ticks arrive at ~10 Hz; we redraw every
  // animation frame so splash rings can interpolate smoothly.
  let latest = null;
  // Active splash animations: { x, y, startedAt }.
  const splashes = [];
  // Scrolling event log.
  const eventLog = [];

  function setStatus(connected, message) {
    statusEl.textContent = message;
    statusEl.classList.toggle("status-connected", connected);
    statusEl.classList.toggle("status-disconnected", !connected);
  }

  function fitCanvas() {
    // Match the canvas' device-pixel resolution to its layout size so the lines aren't
    // fuzzy on hi-DPI displays. Coordinates remain in MAP units; we re-apply the scale
    // every frame inside `draw`.
    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    canvas.width = Math.max(1, Math.round(rect.width * dpr));
    canvas.height = Math.max(1, Math.round(rect.height * dpr));
  }
  window.addEventListener("resize", fitCanvas);
  fitCanvas();

  // View toggle: split (canvas + sidebar) vs. full (canvas fills the window).
  const mainEl = document.querySelector("main");
  const viewToggleBtn = document.getElementById("view-toggle");
  if (mainEl && viewToggleBtn) {
    viewToggleBtn.addEventListener("click", () => {
      const isFull = mainEl.classList.toggle("layout-full");
      mainEl.classList.toggle("layout-split", !isFull);
      viewToggleBtn.setAttribute("aria-pressed", isFull ? "true" : "false");
      viewToggleBtn.textContent = isFull ? "Split view" : "Fit battlefield";
      // The canvas just changed size; re-sync its pixel buffer.
      fitCanvas();
    });
  }

  function connect() {
    const url = `ws://${location.host}/spectate`;
    setStatus(false, "connecting…");
    let ws;
    try {
      ws = new WebSocket(url);
    } catch (e) {
      setStatus(false, "ws unavailable");
      return;
    }
    ws.onopen = () => setStatus(true, "live");
    ws.onclose = () => {
      setStatus(false, "disconnected — retrying");
      setTimeout(connect, 1500);
    };
    ws.onerror = () => {
      // onclose will fire next; let it handle the retry.
    };
    ws.onmessage = (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch (e) {
        console.warn("bad world frame", e);
        return;
      }
      if (msg && msg.type === "world") handleWorld(msg);
    };
  }

  function handleWorld(world) {
    latest = world;
    tickEl.textContent = world.tick;

    if (Array.isArray(world.events)) {
      for (const ev of world.events) {
        if (ev.type === "shell_splash" && Array.isArray(ev.pos)) {
          splashes.push({ x: ev.pos[0], y: ev.pos[1], startedAt: performance.now() });
        }
        const line = formatEvent(world.tick, ev);
        if (line) {
          eventLog.unshift(line);
          if (eventLog.length > MAX_EVENTS) eventLog.length = MAX_EVENTS;
        }
      }
      renderEvents();
    }

    renderBots(world.tick, world.ships || []);
  }

  function formatEvent(tick, ev) {
    if (!ev || !ev.type) return null;
    const t = `[t${tick}]`;
    switch (ev.type) {
      case "hit":
        return `${t} hit ${ev.ship_id} (-${ev.amount})`;
      case "shell_splash":
        return `${t} splash @ (${ev.pos[0].toFixed(0)}, ${ev.pos[1].toFixed(0)})`;
      case "death":
        return `${t} ${ev.ship_id} destroyed`;
      default:
        return null;
    }
  }

  function renderBots(tick, ships) {
    // Update the in-memory map: refresh entries we saw this tick, mark the rest as
    // disconnected so the operator can tell when a bot drops mid-match. Order is the
    // first-seen order, which lines up with the server's `BotId` ordering.
    for (const ship of ships) {
      const prev = botCards.get(ship.id);
      botCards.set(ship.id, {
        ship,
        lastSeenTick: tick,
        firstSeenOrder: prev ? prev.firstSeenOrder : botCards.size,
        connected: true,
      });
    }
    for (const [id, card] of botCards) {
      if (card.lastSeenTick !== tick) {
        card.connected = false;
      }
    }

    const ordered = Array.from(botCards.values()).sort(
      (a, b) => a.firstSeenOrder - b.firstSeenOrder,
    );

    botsEl.innerHTML = "";
    for (const card of ordered) {
      botsEl.appendChild(buildBotCard(card));
    }
  }

  function buildBotCard(card) {
    const ship = card.ship;
    const li = document.createElement("li");
    li.className = "bot";
    if (!ship.alive) li.classList.add("dead");
    if (!card.connected) li.classList.add("disconnected");

    // Header row: color swatch, name, status pill.
    const header = document.createElement("div");
    header.className = "bot-header";

    const swatch = document.createElement("span");
    swatch.className = "bot-swatch";
    swatch.style.background = colorFor(ship.bot_name);

    const name = document.createElement("span");
    name.className = "bot-name";
    name.textContent = ship.bot_name;
    name.title = `${ship.bot_name} (${ship.id})`;

    const status = document.createElement("span");
    const { label: statusLabel, cls: statusCls } = statusFor(ship, card.connected);
    status.className = `bot-status ${statusCls}`;
    status.textContent = statusLabel;

    header.appendChild(swatch);
    header.appendChild(name);
    header.appendChild(status);

    // Resource meters (HP + ammo).
    const meters = document.createElement("div");
    meters.className = "bot-meters";
    meters.appendChild(meterRow("HP", ship.hp, MAX_HP, hpColor(ship.hp), `${ship.hp}/${MAX_HP}`));
    meters.appendChild(meterRow("AMMO", ship.ammo, MAX_AMMO, "#6cb1ff", `${ship.ammo}/${MAX_AMMO}`));

    // Control sliders (throttle + rudder) and speed readout.
    const controls = document.createElement("div");
    controls.className = "bot-controls";
    controls.appendChild(sliderRow("THR", ship.throttle, signedFmt(ship.throttle)));
    controls.appendChild(speedRow(ship.speed));
    controls.appendChild(sliderRow("RUD", ship.rudder, signedFmt(ship.rudder)));

    // Footer: commands-per-second + sensor mode.
    const footer = document.createElement("div");
    footer.className = "bot-footer";
    const cps = document.createElement("span");
    cps.className = "bot-stat";
    const cpsValue = typeof ship.commands_per_sec === "number" ? ship.commands_per_sec : 0;
    cps.innerHTML = `<span class="bot-stat-key">CPS</span> ${cpsValue.toFixed(0)}/s`;
    const sensor = document.createElement("span");
    sensor.className = `bot-stat bot-sensor-${ship.sensor_mode || "passive"}`;
    sensor.innerHTML = `<span class="bot-stat-key">SENSOR</span> ${ship.sensor_mode || "—"}`;
    footer.appendChild(cps);
    footer.appendChild(sensor);

    li.appendChild(header);
    li.appendChild(meters);
    li.appendChild(controls);
    li.appendChild(footer);
    return li;
  }

  function statusFor(ship, connected) {
    if (!connected) return { label: "disconnected", cls: "pill-bad" };
    if (!ship.alive) return { label: "destroyed", cls: "pill-bad" };
    if (!ship.ready) return { label: "lobby", cls: "pill-muted" };
    return { label: "live", cls: "pill-good" };
  }

  function hpColor(hp) {
    if (hp > 60) return "var(--good)";
    if (hp > 25) return "#f4d35e";
    return "var(--bad)";
  }

  function meterRow(label, value, max, fill, valueText) {
    const row = document.createElement("div");
    row.className = "meter-row";
    const key = document.createElement("span");
    key.className = "meter-key";
    key.textContent = label;
    const track = document.createElement("div");
    track.className = "meter-track";
    const bar = document.createElement("div");
    bar.className = "meter-fill";
    const pct = Math.max(0, Math.min(1, value / max));
    bar.style.width = `${pct * 100}%`;
    bar.style.background = fill;
    track.appendChild(bar);
    const valueEl = document.createElement("span");
    valueEl.className = "meter-value";
    valueEl.textContent = valueText;
    row.appendChild(key);
    row.appendChild(track);
    row.appendChild(valueEl);
    return row;
  }

  function sliderRow(label, value, valueText) {
    const row = document.createElement("div");
    row.className = "meter-row";
    const key = document.createElement("span");
    key.className = "meter-key";
    key.textContent = label;
    const track = document.createElement("div");
    track.className = "slider-track";
    const center = document.createElement("div");
    center.className = "slider-center";
    track.appendChild(center);
    const fill = document.createElement("div");
    fill.className = "slider-fill";
    const clamped = Math.max(-1, Math.min(1, value || 0));
    const widthPct = Math.abs(clamped) * 50;
    if (clamped >= 0) {
      fill.style.left = "50%";
    } else {
      fill.style.left = `${50 - widthPct}%`;
    }
    fill.style.width = `${widthPct}%`;
    fill.style.background = clamped >= 0 ? "#6cb1ff" : "#ef9a4a";
    track.appendChild(fill);
    const valueEl = document.createElement("span");
    valueEl.className = "meter-value";
    valueEl.textContent = valueText;
    row.appendChild(key);
    row.appendChild(track);
    row.appendChild(valueEl);
    return row;
  }

  function speedRow(speed) {
    const row = document.createElement("div");
    row.className = "meter-row";
    const key = document.createElement("span");
    key.className = "meter-key";
    key.textContent = "SPD";
    const track = document.createElement("div");
    track.className = "slider-track";
    const center = document.createElement("div");
    center.className = "slider-center";
    track.appendChild(center);
    const fill = document.createElement("div");
    fill.className = "slider-fill";
    const ratio =
      speed >= 0 ? speed / MAX_FORWARD_SPEED : speed / MAX_REVERSE_SPEED;
    const clamped = Math.max(-1, Math.min(1, ratio));
    const widthPct = Math.abs(clamped) * 50;
    if (clamped >= 0) {
      fill.style.left = "50%";
    } else {
      fill.style.left = `${50 - widthPct}%`;
    }
    fill.style.width = `${widthPct}%`;
    fill.style.background = clamped >= 0 ? "var(--good)" : "#ef9a4a";
    track.appendChild(fill);
    const valueEl = document.createElement("span");
    valueEl.className = "meter-value";
    valueEl.textContent = `${speed.toFixed(1)} u/s`;
    row.appendChild(key);
    row.appendChild(track);
    row.appendChild(valueEl);
    return row;
  }

  function signedFmt(v) {
    if (typeof v !== "number" || Number.isNaN(v)) return "—";
    return (v >= 0 ? "+" : "") + v.toFixed(2);
  }

  function renderEvents() {
    eventsEl.innerHTML = "";
    for (const line of eventLog) {
      const li = document.createElement("li");
      li.textContent = line;
      eventsEl.appendChild(li);
    }
  }

  function draw() {
    requestAnimationFrame(draw);
    const w = canvas.width;
    const h = canvas.height;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, w, h);

    // World→canvas scale: keep aspect ratio by using the smaller axis.
    const scale = Math.min(w / MAP_WIDTH, h / MAP_HEIGHT);
    const offX = (w - MAP_WIDTH * scale) / 2;
    const offY = (h - MAP_HEIGHT * scale) / 2;
    ctx.setTransform(scale, 0, 0, scale, offX, offY);

    // Map bounds box.
    ctx.lineWidth = 1 / scale;
    ctx.strokeStyle = "rgba(255, 255, 255, 0.18)";
    ctx.strokeRect(0, 0, MAP_WIDTH, MAP_HEIGHT);

    if (!latest) return;

    // Active radar rings beneath everything else.
    for (const ship of latest.ships || []) {
      if (!ship.alive) continue;
      if (ship.sensor_mode !== "active") continue;
      ctx.beginPath();
      ctx.arc(ship.pos[0], ship.pos[1], ACTIVE_RADAR_RANGE, 0, Math.PI * 2);
      const col = colorFor(ship.bot_name);
      ctx.fillStyle = withAlpha(col, 0.06);
      ctx.fill();
      ctx.strokeStyle = withAlpha(col, 0.35);
      ctx.lineWidth = 1.5 / scale;
      ctx.stroke();
    }

    // Shells as small dots.
    for (const shell of latest.shells || []) {
      ctx.beginPath();
      ctx.arc(shell.pos[0], shell.pos[1], 3, 0, Math.PI * 2);
      ctx.fillStyle = "#fff5b8";
      ctx.fill();
      // Faint trail in the direction opposite velocity.
      const vx = shell.vel[0];
      const vy = shell.vel[1];
      const speed = Math.hypot(vx, vy) || 1;
      ctx.beginPath();
      ctx.moveTo(shell.pos[0], shell.pos[1]);
      ctx.lineTo(shell.pos[0] - (vx / speed) * 18, shell.pos[1] - (vy / speed) * 18);
      ctx.strokeStyle = "rgba(255, 245, 184, 0.4)";
      ctx.lineWidth = 1.5 / scale;
      ctx.stroke();
    }

    // Splash rings (animated).
    const now = performance.now();
    for (let i = splashes.length - 1; i >= 0; i--) {
      const sp = splashes[i];
      const age = now - sp.startedAt;
      if (age > SPLASH_DRAW_MS) {
        splashes.splice(i, 1);
        continue;
      }
      const t = age / SPLASH_DRAW_MS;
      const r = 6 + t * 30;
      ctx.beginPath();
      ctx.arc(sp.x, sp.y, r, 0, Math.PI * 2);
      ctx.strokeStyle = `rgba(255, 240, 180, ${1 - t})`;
      ctx.lineWidth = 2 / scale;
      ctx.stroke();
    }

    // Ships.
    for (const ship of latest.ships || []) {
      drawShip(ship, scale);
    }
  }

  function drawShip(ship, scale) {
    const color = colorFor(ship.bot_name);
    const x = ship.pos[0];
    const y = ship.pos[1];

    // Compass heading: 0° = north (-y), 90° = east (+x).
    const rad = (ship.heading_deg * Math.PI) / 180;
    const dirX = Math.sin(rad);
    const dirY = -Math.cos(rad);
    const perpX = -dirY;
    const perpY = dirX;

    // Triangle pointing along heading. Nose 1.4*r ahead, tail corners 0.8*r behind.
    const nose = [x + dirX * SHIP_RADIUS * 1.4, y + dirY * SHIP_RADIUS * 1.4];
    const tailL = [
      x - dirX * SHIP_RADIUS * 0.8 + perpX * SHIP_RADIUS * 0.9,
      y - dirY * SHIP_RADIUS * 0.8 + perpY * SHIP_RADIUS * 0.9,
    ];
    const tailR = [
      x - dirX * SHIP_RADIUS * 0.8 - perpX * SHIP_RADIUS * 0.9,
      y - dirY * SHIP_RADIUS * 0.8 - perpY * SHIP_RADIUS * 0.9,
    ];

    ctx.beginPath();
    ctx.moveTo(nose[0], nose[1]);
    ctx.lineTo(tailL[0], tailL[1]);
    ctx.lineTo(tailR[0], tailR[1]);
    ctx.closePath();
    ctx.fillStyle = ship.alive ? color : withAlpha(color, 0.25);
    ctx.fill();
    ctx.strokeStyle = ship.alive ? "rgba(0,0,0,0.45)" : "rgba(0,0,0,0.2)";
    ctx.lineWidth = 1 / scale;
    ctx.stroke();

    // Name + HP bar above the ship. Rendered in screen-pixel sizes by un-scaling.
    const labelOffsetY = -SHIP_RADIUS * 1.8;
    ctx.save();
    ctx.translate(x, y + labelOffsetY);
    // Counter the world scale so text stays a constant on-screen size.
    ctx.scale(1 / scale, 1 / scale);
    ctx.font = "11px -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif";
    ctx.textAlign = "center";
    ctx.fillStyle = ship.alive ? "rgba(255,255,255,0.95)" : "rgba(255,255,255,0.4)";
    ctx.fillText(ship.bot_name, 0, 0);

    if (ship.alive) {
      const barW = 36;
      const barH = 4;
      ctx.fillStyle = "rgba(255,255,255,0.15)";
      ctx.fillRect(-barW / 2, 4, barW, barH);
      const pct = Math.max(0, Math.min(1, ship.hp / 100));
      const hpColor = ship.hp > 60 ? "#58d68d" : ship.hp > 25 ? "#f4d35e" : "#ef6b6b";
      ctx.fillStyle = hpColor;
      ctx.fillRect(-barW / 2, 4, barW * pct, barH);
    }
    ctx.restore();
  }

  function withAlpha(hex, a) {
    // Accept #rgb or #rrggbb.
    let h = hex.replace("#", "");
    if (h.length === 3) h = h.split("").map((c) => c + c).join("");
    const r = parseInt(h.slice(0, 2), 16);
    const g = parseInt(h.slice(2, 4), 16);
    const b = parseInt(h.slice(4, 6), 16);
    return `rgba(${r}, ${g}, ${b}, ${a})`;
  }

  connect();
  requestAnimationFrame(draw);
})();
