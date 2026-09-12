<script lang="ts">
  import type { BotCardState } from '../lib/worldFrame';
  import { colorFor } from '../lib/palette';
  import { hpColor } from '../lib/hpColor';
  import { MAX_AMMO, MAX_HP } from '../lib/constants';
  import { chipsForShip } from '../lib/powerupHud';
  import MeterRow from './MeterRow.svelte';
  import { room, kickBot, actionNotice } from '../stores/admin';
  import { selectedTeam, projector } from '../stores';
  let { card }: { card: BotCardState } = $props();
  const ship = $derived(card.ship);
  const adminBot = $derived($room?.bots.find(b => b.ship_id === ship.id));
  const selected = $derived($selectedTeam === ship.id);
  const maxHp = $derived($room?.config.hull_hp ?? MAX_HP);
  const maxAmmo = $derived($room?.config.max_ammo ?? MAX_AMMO);
  const status = $derived(adminBot?.forfeited ? 'Forfeited' : !ship.alive ? 'Destroyed' : adminBot?.connected === false ? 'Disconnected' : adminBot?.connected === true ? 'Live' : 'Connection unknown');
  let confirming = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  async function forfeit() {
    if (!adminBot || busy || confirming !== $room?.match_id) return;
    busy = true; error = null;
    try { await kickBot(adminBot.bot_id); confirming = null; actionNotice.set(`${ship.bot_name} forfeited the match.`); }
    catch (e) { error = e instanceof Error ? e.message : 'Could not forfeit team'; }
    finally { busy = false; }
  }
</script>
<li class="bot" class:selected class:dead={!ship.alive}>
  <button class="team-select" aria-expanded={selected} onclick={() => selectedTeam.set(selected ? null : ship.id)}>
    <span class="bot-swatch" style:background={colorFor(ship.bot_name)}></span>
    <span class="bot-name" title={ship.bot_name}>{ship.id.replace('s_', '')}. {ship.bot_name}</span>
    <span class="bot-status" class:pill-bad={!ship.alive || adminBot?.connected === false}>{status}</span>
  </button>
  <div class="compact-meters"><MeterRow label="HP" value={ship.hp} max={maxHp} fill={hpColor(ship.hp / maxHp * 100)} valueText={`${ship.hp}/${maxHp}`} /><span class="ammo">Ammo {ship.ammo}/{maxAmmo}</span></div>
  {#if selected}
    <div class="team-details">
      <span>Speed {ship.speed.toFixed(1)} u/s</span><span>Rudder {ship.rudder.toFixed(2)}</span>
      <span>Throttle {ship.throttle.toFixed(2)}</span><span>Sensor {ship.sensor_mode}</span>
      <span>Commands {ship.commands_per_sec.toFixed(0)}/sim s</span>
      {#each chipsForShip(ship) as chip (chip.id)}<span>{chip.label} · {chip.state === 'active' && $room?.tick_hz ? `${(chip.activeTicksLeft / $room.tick_hz).toFixed(1)}s left` : chip.state}</span>{/each}
      {#if adminBot?.disconnect_reason}<p class="config-err">{adminBot.disconnect_reason}</p>{/if}
    </div>
    {#if adminBot?.connected && $room?.state === 'running' && $room.capabilities?.manage_match && !$projector}
      {#if confirming !== null}
        <div class="forfeit-confirm" role="alertdialog" aria-label="Confirm team forfeit">
          <p>Forfeit {ship.bot_name} in match {confirming}? The team is eliminated and disconnected.</p>
          <button class="topbar-btn danger" disabled={busy || confirming !== $room.match_id} onclick={forfeit}>{busy ? 'Forfeiting…' : 'Confirm forfeit'}</button>
          <button class="topbar-btn" disabled={busy} onclick={() => confirming = null}>Cancel</button>
        </div>
      {:else}<button class="topbar-btn danger" onclick={() => confirming = $room?.match_id ?? ''}>Forfeit team</button>{/if}
    {/if}
    {#if error}<p class="config-err" role="alert">{error}</p>{/if}
  {/if}
</li>
