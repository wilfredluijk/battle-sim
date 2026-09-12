<script lang="ts">
  import { appMode, projector } from '../stores';
  import { room, roomError, roomUpdatedAt, abortMatch, actionNotice, configDraft } from '../stores/admin';
  import { replayPlaying } from '../stores/replay';
  import LoginBox from './LoginBox.svelte';
  let { screen }: { screen: string } = $props();
  let abortBusy = $state(false);
  let confirmAbort = $state<string | null>(null);
  let error = $state<string | null>(null);
  let now = $state(Date.now());
  $effect(() => { const id = setInterval(() => now = Date.now(), 1000); return () => clearInterval(id); });
  const stale = $derived($roomUpdatedAt > 0 && now - $roomUpdatedAt > 5000);
  const viewed = $derived(screen === 'replay-viewer' ? `Replay · ${$replayPlaying ? 'playing' : 'paused'}` :
    screen === 'replay-browser' ? 'Replay library' : screen === 'monte-carlo' ? 'Offline analysis' :
    screen === 'battle' ? ($room?.replay_mode ? 'Server replay' : 'Live match') : screen === 'results' ? 'Results' : screen === 'settings' ? 'Settings' : 'Lobby');
  const freshness = $derived($roomError ? 'Disconnected · retrying' : stale ? 'Stale server data' : !$room ? 'Connecting…' : 'Server connected');
  async function handleAbort() {
    if (abortBusy || confirmAbort !== $room?.match_id) return;
    abortBusy = true; error = null;
    try { await abortMatch(); confirmAbort = null; actionNotice.set('Match aborted. Results are available.'); }
    catch (e) { error = e instanceof Error ? e.message : 'Could not abort match'; }
    finally { abortBusy = false; }
  }
</script>
<header class="topbar">
  <span class="topbar-title">Naval Battle</span>
  <nav class="primary-nav" aria-label="Main navigation">
    <button class="topbar-btn" aria-current={screen === 'lobby' ? 'page' : undefined} onclick={() => appMode.set('lobby')}>Lobby</button>
    <button class="topbar-btn" aria-current={screen === 'battle' ? 'page' : undefined} onclick={() => appMode.set('live')}>Live match</button>
    <button class="topbar-btn" aria-current={$appMode === 'results' ? 'page' : undefined} onclick={() => appMode.set('results')}>Results</button>
    <button class="topbar-btn" aria-current={$appMode.startsWith('replay') ? 'page' : undefined} onclick={() => appMode.set('replay-browser')}>Replays</button>
  </nav>
  <span class="topbar-spacer"></span>
  <div class="topbar-controls">
    <button class="topbar-btn quiet" onclick={() => appMode.set('settings')}>Settings{$configDraft ? ' •' : ''}</button>
    {#if $room?.capabilities?.monte_carlo}<button class="topbar-btn quiet" onclick={() => appMode.set('monte-carlo')}>Analysis</button>{/if}
    <button class="topbar-btn" aria-pressed={$projector} onclick={() => projector.update(v => !v)}>Projector</button>
    <LoginBox />
  </div>
</header>
<div class="session-strip">
  <strong>{viewed}</strong>
  <span class:config-err={!!$roomError || stale} role="status">{freshness}</span>
  {#if $room?.state === 'running' && $appMode !== 'live'}<span>Match running in background</span>{/if}
  {#if $room?.state === 'running' && $room.capabilities?.manage_match && !$projector}
    <button class="topbar-btn danger" onclick={() => { confirmAbort = $room?.match_id ?? ''; error = null; }}>Abort match</button>
  {/if}
  {#if $actionNotice}<span role="status">{$actionNotice}</span><button class="topbar-btn quiet" aria-label="Dismiss notification" onclick={() => actionNotice.set(null)}>×</button>{/if}
</div>
{#if confirmAbort !== null}
  <div class="confirmation" role="alertdialog" aria-label="Confirm match abort">
    <strong>Abort match {confirmAbort} in {$room?.room}?</strong>
    <span>This ends the match for every team with no winner.</span>
    <button class="topbar-btn danger" disabled={abortBusy || confirmAbort !== $room?.match_id || $room?.state !== 'running'} onclick={handleAbort}>{abortBusy ? 'Aborting…' : 'Confirm abort'}</button>
    <button class="topbar-btn" disabled={abortBusy} onclick={() => confirmAbort = null}>Cancel</button>
    {#if error}<span class="config-err" role="alert">{error}</span>{/if}
  </div>
{/if}
