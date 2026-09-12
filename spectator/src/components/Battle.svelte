<script lang="ts">
  import Battlefield from './Battlefield.svelte';
  import Sidebar from './Sidebar.svelte';
  import { view, tick, latestWorld, displayOptions, worldUpdatedAt, connection } from '../stores';
  import { room } from '../stores/admin';
  import { clockText } from '../lib/presentation';
  let now = $state(Date.now());
  $effect(() => { const id = setInterval(() => now = Date.now(), 1000); return () => clearInterval(id); });
  const hz = $derived($room?.tick_hz);
  const stale = $derived(!$worldUpdatedAt || now - $worldUpdatedAt > 3000);
</script>
<main class="battle" class:layout-full={$view === 'full'}>
  <header class="match-header">
    <div><h1>{$room?.round?.name ?? $room?.room ?? 'Match'}</h1><span class="pm-sub">Match {$room?.match_id || '—'} · tick {$tick}</span></div>
    <div class="match-clock">{hz ? clockText($tick / hz) : '—'} <small>elapsed</small></div>
    <div class="match-clock">{hz && $room?.match_timeout_ticks ? clockText(($room.match_timeout_ticks - $tick) / hz) : '—'} <small>remaining</small></div>
    <strong>{$latestWorld?.ships.filter(s => s.alive).length ?? 0} survivors</strong>
    <span role="status" class:config-err={stale || !$connection.connected}>{!$connection.connected ? 'Stream disconnected' : stale ? 'Stale match data' : 'Live data'}</span>
  </header>
  <div class="battle-stage"><Battlefield /></div>
  <Sidebar />
  <footer class="battle-toolbar">
    <label><input type="checkbox" bind:checked={$displayOptions.radar} /> Radar rings</label>
    <label><input type="checkbox" bind:checked={$displayOptions.labels} /> Labels</label>
    <label><input type="checkbox" bind:checked={$displayOptions.trails} /> Trails</label>
    <span class="pm-sub">▲ ship · ● shell · dashed hull: decoy · grey area: smoke</span>
    <button class="topbar-btn" aria-pressed={$view === 'full'} onclick={() => view.update(v => v === 'full' ? 'split' : 'full')}>{$view === 'full' ? 'Show roster' : 'Expand map'}</button>
  </footer>
</main>
