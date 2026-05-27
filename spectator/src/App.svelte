<script lang="ts">
  import { onDestroy } from 'svelte';
  import Topbar from './components/Topbar.svelte';
  import Battle from './components/Battle.svelte';
  import MonteCarloPanel from './components/MonteCarloPanel.svelte';
  import PreMatch from './components/PreMatch.svelte';
  import Report from './components/Report.svelte';
  import ReplayBrowser from './components/ReplayBrowser.svelte';
  import ReplayViewer from './components/ReplayViewer.svelte';
  import { appMode, startSpectator } from './stores';
  import {
    startControlPlane,
    startMonteCarloPolling,
    room,
    report,
    showReport,
  } from './stores/admin';

  const teardownSpectator = startSpectator();
  const teardownControl = startControlPlane();
  const teardownMc = startMonteCarloPolling();
  onDestroy(() => {
    teardownSpectator();
    teardownControl();
    teardownMc();
  });

  // Which top-level screen is visible. The replay / monte-carlo modes are independent
  // surfaces and take precedence over the live screens; otherwise the battle overview
  // shows while a match runs, falling back to the lobby (or the post-battle report).
  const screen = $derived.by(() => {
    if ($appMode === 'replay-browser') return 'replay-browser';
    if ($appMode === 'replay-viewer') return 'replay-viewer';
    if ($appMode === 'monte-carlo') return 'monte-carlo';
    if ($room?.state === 'running') return 'battle';
    if ($showReport && $report) return 'report';
    return 'prematch';
  });
</script>

<Topbar />
{#if screen === 'battle'}
  <Battle />
{:else if screen === 'report'}
  <Report />
{:else if screen === 'replay-browser'}
  <ReplayBrowser />
{:else if screen === 'replay-viewer'}
  <ReplayViewer />
{:else if screen === 'monte-carlo'}
  <MonteCarloPanel />
{:else}
  <PreMatch />
{/if}
