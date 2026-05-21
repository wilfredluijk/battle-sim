<script lang="ts">
  import { onDestroy } from 'svelte';
  import Topbar from './components/Topbar.svelte';
  import Battle from './components/Battle.svelte';
  import PreMatch from './components/PreMatch.svelte';
  import Report from './components/Report.svelte';
  import { startSpectator } from './stores';
  import { startControlPlane, room, report, showReport } from './stores/admin';

  const teardownSpectator = startSpectator();
  const teardownControl = startControlPlane();
  onDestroy(() => {
    teardownSpectator();
    teardownControl();
  });

  // Which top-level screen is visible. The battle overview is only shown while a match is
  // actually running — otherwise the landing screen is the pre-match lobby (or the
  // post-battle report once a match has finished).
  const screen = $derived.by(() => {
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
{:else}
  <PreMatch />
{/if}
