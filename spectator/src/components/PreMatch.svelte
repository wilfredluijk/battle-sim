<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { countdownSound, resetLiveCues } from '../stores/presenter';
  import { room, roomError, configDraft, startMatch, kickBot, actionNotice, training } from '../stores/admin';
  import { appMode } from '../stores';
  import { colorFor } from '../lib/palette';
  import { clockText } from '../lib/presentation';
  import ConnectionCheck from './ConnectionCheck.svelte';
  import HealthPanel from './HealthPanel.svelte';
  const activeSession = $derived($training?.sessions.find(s => s.id === $training.active_session));
  const bots = $derived(($room?.bots ?? []).filter(b => b.connected !== false));
  const expected = $derived($room?.expected_teams ?? []);
  const missing = $derived(expected.filter(name => !bots.some(b => b.name === name)));
  const allReady = $derived(bots.length > 0 && bots.every(b => b.ready));
  const canStart = $derived(allReady && !$configDraft && $room?.state === 'lobby' && !$roomError && !!$room?.capabilities?.manage_match);
  let starting = $state(false);
  let kicking = $state<string | null>(null);
  let error = $state<string | null>(null);
  let countdown = $state<number | null>(null);
  $effect(() => {
    if (countdown === null) return;
    if (!canStart) { countdown = null; return; }
    if (countdown === 0) { countdown = null; void begin(); return; }
    untrack(countdownSound);
    const timer = setTimeout(() => countdown = countdown === null ? null : countdown - 1, 1000);
    return () => { clearTimeout(timer); resetLiveCues(); };
  });
  $effect(() => {
    const hide = () => { if (document.hidden) countdown = null; };
    document.addEventListener('visibilitychange', hide);
    return () => document.removeEventListener('visibilitychange', hide);
  });
  onDestroy(resetLiveCues);
  async function begin() {
    if (!canStart || starting) return;
    starting = true; error = null;
    try { await startMatch(); appMode.set('live'); actionNotice.set('Match started.'); }
    catch (e) { error = e instanceof Error ? e.message : 'Could not start match'; }
    finally { starting = false; }
  }
  async function disconnect(id: string, name: string) {
    if (kicking) return;
    kicking = id; error = null;
    try { await kickBot(id); actionNotice.set(`${name} disconnected from the lobby.`); }
    catch (e) { error = e instanceof Error ? e.message : 'Could not disconnect team'; }
    finally { kicking = null; }
  }
  let copied = $state('');
  async function copyAddress() {
    try { await navigator.clipboard.writeText(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/bot`); copied = 'Connection address copied.'; }
    catch { copied = 'Copy unavailable. Use the address shown below.'; }
  }
</script>
<main class="prematch">
  <section class="pm-panel">
    <header class="pm-head"><h1>Team lobby</h1><span class="admin-badge">{$room?.capabilities?.tournament ? 'Tournament' : 'Training'}</span></header>
    <p class="pm-sub">{activeSession ? `${activeSession.name} · Next: ${activeSession.next_round}` : "No training session selected"} <button class="topbar-btn" onclick={() => appMode.set("sessions")}>Manage session</button></p>
    <div class="lobby-counts"><span><strong>{expected.length || '—'}</strong> expected</span><span><strong>{bots.length}</strong> connected</span><span><strong>{bots.filter(b => b.ready).length}</strong> ready</span></div>
    {#if $room?.roster_error}<p class="config-err" role="alert">Expected roster unavailable: {$room.roster_error}</p>{/if}
    <ul class="pm-bots">
      {#each bots as b (b.bot_id)}
        <li class="pm-bot"><span class="bot-swatch" style:background={colorFor(b.name)}></span><div class="team-identity"><strong title={b.name}>{b.name}</strong><small>{b.ready ? `Acknowledged rules ${$room?.config_hash?.slice(0, 10) ?? ''}` : b.readiness_blocker ?? 'Waiting for readiness'}</small></div><span class="bot-status" class:pill-good={b.ready}>{b.ready ? 'Ready' : 'Waiting'}</span>
          {#if $room?.state === 'lobby'}<button class="topbar-btn danger" disabled={kicking !== null || countdown !== null} onclick={() => disconnect(b.bot_id, b.name)}>{kicking === b.bot_id ? 'Disconnecting…' : 'Disconnect'}</button>{/if}
        </li>
      {/each}
      {#each missing as name (name)}<li class="pm-bot missing"><span class="bot-swatch"></span><strong>{name}</strong><span class="bot-status">Not connected</span></li>{/each}
    </ul>
    {#if !bots.length}<p class="pm-sub">Waiting for teams to connect and acknowledge the rules.</p>{/if}
    {#if $configDraft}<p class="draft-summary">Unapplied rules. <button class="topbar-btn" onclick={() => appMode.set('settings')}>Apply or discard draft</button></p>{/if}
    {#if missing.length}<p class="config-note">{missing.length} expected team{missing.length === 1 ? ' is' : 's are'} missing. Starting includes the connected teams only.</p>{/if}
    {#if countdown !== null}<div class="countdown" role="status">Starting in {countdown}… <button class="topbar-btn" onclick={() => countdown = null}>Cancel countdown</button></div>
    {:else}<button class="pm-start" disabled={starting || !canStart} onclick={() => countdown = 5}>{starting ? 'Starting…' : 'Start match · 5 second countdown'}</button>{/if}
    {#if $room?.state !== 'lobby'}<p class="config-note">{$room?.state === 'running' ? 'A match is running. Open Live match to follow it.' : 'Preparing the next round…'}</p>{/if}
    {#if error}<p class="config-err" role="alert">{error}</p>{/if}
  </section>
  <div class="lobby-side">
    <section class="pm-panel"><h2>Active rules</h2><dl class="rules-summary"><dt>Map</dt><dd>{$room?.map?.width} × {$room?.map?.height} units</dd><dt>Time limit</dt><dd>{$room?.tick_hz && $room.match_timeout_ticks ? clockText($room.match_timeout_ticks / $room.tick_hz) : 'Unknown'}</dd><dt>Command deadline</dt><dd>{$room?.tick_deadline_ms ?? '—'} ms</dd><dt>Hull / ammo</dt><dd>{$room?.config.hull_hp} HP / {$room?.config.max_ammo}</dd></dl><button class="topbar-btn" onclick={() => appMode.set('settings')}>Review settings</button></section>
    <section class="pm-panel"><h2>Team connection check</h2><p class="pm-sub">Use protocol 3.0 and the current SDK. Each team connects with its privately supplied credential and acknowledges the active rules.</p><code class="server-address">{typeof location !== 'undefined' ? `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/bot` : ''}</code><button class="topbar-btn" onclick={copyAddress}>Copy connection address</button>{#if copied}<p role="status" class="pm-sub">{copied}</p>{/if}<p class="config-note">Ready confirms the handshake and rules acknowledgement. Run a short practice match to check command delivery at this venue.</p></section>
    <ConnectionCheck />
    <HealthPanel />
  </div>
</main>
