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

  const canvas = document.getElementById("board");
  const ctx = canvas.getContext("2d");
  const statusEl = document.getElementById("status");
  const tickEl = document.getElementById("tick");
  const playersEl = document.getElementById("players");
  const eventsEl = document.getElementById("events");

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

    renderPlayers(world.ships || []);
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

  function renderPlayers(ships) {
    playersEl.innerHTML = "";
    for (const ship of ships) {
      const li = document.createElement("li");
      if (!ship.alive) li.classList.add("dead");

      const swatch = document.createElement("span");
      swatch.className = "player-color";
      swatch.style.background = colorFor(ship.bot_name);

      const name = document.createElement("span");
      name.className = "player-name";
      name.textContent = ship.bot_name;
      name.title = `${ship.bot_name} (${ship.id})`;

      const meta = document.createElement("span");
      meta.className = "player-meta";
      meta.textContent = `${ship.hp} HP`;

      const hpWrap = document.createElement("div");
      hpWrap.className = "player-hp-wrap";
      const hpFill = document.createElement("div");
      hpFill.className = "player-hp-fill";
      const pct = Math.max(0, Math.min(100, ship.hp));
      hpFill.style.width = `${pct}%`;
      hpFill.style.background =
        ship.hp > 60 ? "var(--good)" : ship.hp > 25 ? "#f4d35e" : "var(--bad)";
      hpWrap.appendChild(hpFill);

      li.appendChild(swatch);
      const middle = document.createElement("div");
      middle.style.display = "flex";
      middle.style.flexDirection = "column";
      middle.style.gap = "3px";
      middle.style.minWidth = "0";
      middle.appendChild(name);
      middle.appendChild(hpWrap);
      li.appendChild(middle);
      li.appendChild(meta);
      playersEl.appendChild(li);
    }
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
