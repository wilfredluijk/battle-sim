<script lang="ts">
  import { onDestroy } from 'svelte';
  import SessionPanel from './components/SessionPanel.svelte';
  import Topbar from './components/Topbar.svelte';
  import Battle from './components/Battle.svelte';
  import MonteCarloPanel from './components/MonteCarloPanel.svelte';
  import PreMatch from './components/PreMatch.svelte';
  import Report from './components/Report.svelte';
  import ReplayBrowser from './components/ReplayBrowser.svelte';
  import ReplayViewer from './components/ReplayViewer.svelte';
  import LoginBox from './components/LoginBox.svelte';
  import { exitReplay } from './stores/replay';
  import { watchSoundVisibility } from './stores/presenter';
  import ConfigForm from './components/ConfigForm.svelte';
  import { appMode, projector, startSpectator } from './stores';
  import { startControlPlane, startMonteCarloPolling, room, report, showReport, adminToken, sessionExpired, roomError } from './stores/admin';
  const teardownSpectator = startSpectator();
  const teardownControl = startControlPlane();
  const teardownMc = startMonteCarloPolling();
  const teardownSound = watchSoundVisibility();
  onDestroy(() => { teardownSpectator(); teardownControl(); teardownMc(); teardownSound(); });
  $effect(() => { if (!$adminToken) { exitReplay(); projector.set(false); } });
  const screen = $derived.by(() => {
    if ($appMode !== 'live') return $appMode;
    if ($room?.replay_mode || $room?.state === 'running') return 'battle';
    if ($showReport && $report) return 'results';
    return 'lobby';
  });
</script>

<div class="app-shell" class:projector={$projector}>
  {#if !$adminToken}
    <main class="login-screen">
      <section class="login-card">
        <span class="eyebrow">NAVAL BATTLE · TRAINER CONSOLE</span>
        <h1>{$sessionExpired ? 'Session expired' : 'Sign in to your session'}</h1>
        <p class="pm-sub">{$sessionExpired ? 'Sign in again to reconnect to the match.' : 'Manage teams, run matches and explore replays.'}</p>
        <LoginBox />
      </section>
    </main>
  {:else}
    <Topbar {screen} />
    {#if !$room}
      <main class="login-screen"><section class="login-card"><h1>{$roomError ? 'Disconnected' : 'Connecting…'}</h1><p role="status">{$roomError ?? 'Loading the session from the server.'}</p></section></main>
    {:else if screen === 'battle'}<Battle />
    {:else if screen === 'results'}<Report />
    {:else if screen === 'replay-browser'}<ReplayBrowser />
    {:else if screen === 'replay-viewer'}<ReplayViewer />
    {:else if screen === 'monte-carlo' && $room.capabilities?.monte_carlo}<MonteCarloPanel />
    {:else if screen === 'sessions'}<SessionPanel />
    {:else if screen === 'settings'}
      <main class="settings"><section class="pm-panel"><h1>Match settings</h1><ConfigForm /></section></main>
    {:else}<PreMatch />{/if}
  {/if}
</div>
