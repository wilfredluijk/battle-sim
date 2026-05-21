<!-- One bot's status card in the sidebar: identity header, HP+ammo meters, throttle/rudder
     sliders, and a footer with commands-per-second and sensor mode. -->
<script lang="ts">
  import type { BotCardState } from '../lib/worldFrame';
  import { colorFor } from '../lib/palette';
  import { hpColor } from '../lib/hpColor';
  import { MAX_AMMO, MAX_FORWARD_SPEED, MAX_HP, MAX_REVERSE_SPEED } from '../lib/constants';
  import MeterRow from './MeterRow.svelte';
  import SliderRow from './SliderRow.svelte';
  import { room, adminToken, kickBot } from '../stores/admin';

  interface Props {
    card: BotCardState;
  }

  let { card }: Props = $props();

  const ship = $derived(card.ship);
  const swatch = $derived(colorFor(ship.bot_name));

  // Meter scales follow the match's configured parameters when known, falling back to the
  // legacy defaults before the first `GET /api/room` response lands.
  const cfg = $derived($room?.config);
  const maxHp = $derived(cfg?.hull_hp ?? MAX_HP);
  const maxAmmo = $derived(cfg?.max_ammo ?? MAX_AMMO);
  const maxForward = $derived(cfg?.max_forward_speed ?? MAX_FORWARD_SPEED);
  const maxReverse = $derived(cfg?.max_reverse_speed ?? MAX_REVERSE_SPEED);

  type Pill = { label: string; cls: string };
  const status: Pill = $derived.by(() => {
    if (!card.connected) return { label: 'disconnected', cls: 'pill-bad' };
    if (!ship.alive) return { label: 'destroyed', cls: 'pill-bad' };
    if (!ship.ready) return { label: 'lobby', cls: 'pill-muted' };
    return { label: 'live', cls: 'pill-good' };
  });

  function signedFmt(v: number): string {
    if (typeof v !== 'number' || Number.isNaN(v)) return '—';
    return (v >= 0 ? '+' : '') + v.toFixed(2);
  }

  // Speed slider goes from -maxReverse to +maxForward; normalise both sides into [-1, 1].
  const speedRatio = $derived(
    ship.speed >= 0 ? ship.speed / maxForward : ship.speed / maxReverse,
  );

  // Normalise HP to a 0–100 scale so the colour thresholds hold for any configured hull.
  const hpFill = $derived(hpColor(maxHp > 0 ? (ship.hp / maxHp) * 100 : 0));

  // Match the spectator ship id (e.g. "s_3") to the room's bot list so we can issue a kick
  // by bot_id. Hidden when not logged in as admin or no matching bot.
  const adminBotId = $derived.by(() => {
    if (!$adminToken) return null;
    const match = $room?.bots.find((b) => b.ship_id === ship.id);
    return match?.bot_id ?? null;
  });

  function handleKick(): void {
    if (adminBotId) void kickBot(adminBotId);
  }
</script>

<li class="bot" class:dead={!ship.alive} class:disconnected={!card.connected}>
  <div class="bot-header">
    <span class="bot-swatch" style="background: {swatch};"></span>
    <span class="bot-name" title="{ship.bot_name} ({ship.id})">{ship.bot_name}</span>
    <span class="bot-status {status.cls}">{status.label}</span>
    {#if adminBotId}
      <button
        class="bot-kick"
        type="button"
        title="Disconnect {ship.bot_name} ({adminBotId})"
        onclick={handleKick}>Kick</button>
    {/if}
  </div>

  <div class="bot-meters">
    <MeterRow label="HP" value={ship.hp} max={maxHp} fill={hpFill} valueText="{ship.hp}/{maxHp}" />
    <MeterRow label="AMMO" value={ship.ammo} max={maxAmmo} fill="#6cb1ff" valueText="{ship.ammo}/{maxAmmo}" />
  </div>

  <div class="bot-controls">
    <SliderRow label="THR" value={ship.throttle} valueText={signedFmt(ship.throttle)} />
    <SliderRow
      label="SPD"
      value={speedRatio}
      valueText="{ship.speed.toFixed(1)} u/s"
      positiveColor="var(--good)"
    />
    <SliderRow label="RUD" value={ship.rudder} valueText={signedFmt(ship.rudder)} />
  </div>

  <div class="bot-footer">
    <span class="bot-stat">
      <span class="bot-stat-key">CPS</span>{(ship.commands_per_sec ?? 0).toFixed(0)}/s
    </span>
    <span class="bot-stat bot-sensor-{ship.sensor_mode ?? 'passive'}">
      <span class="bot-stat-key">SENSOR</span>{ship.sensor_mode ?? '—'}
    </span>
  </div>
</li>
