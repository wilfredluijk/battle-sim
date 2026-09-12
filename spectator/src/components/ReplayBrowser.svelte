<!-- Replay picker: lists the replays on disk and opens one in the viewer. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchReplays } from '../lib/replayApi';
  import { openReplay, replayError, replayLoading } from '../stores/replay';
  import { appMode } from '../stores';
  import type { ReplaySummary } from '../types/protocol';

  let replays = $state<ReplaySummary[]>([]);
  let search = $state('');
  let outcome = $state('all');
  const filtered = $derived(replays.filter(r => `${r.replay_id} ${r.room} ${r.bots.join(' ')}`.toLowerCase().includes(search.toLowerCase()) && (outcome === 'all' || (r.outcome ?? (r.winner_name ? 'winner' : 'unknown')) === outcome)));
  let loading = $state(true);
  let listError = $state<string | null>(null);

  onMount(async () => {
    try {
      replays = await fetchReplays();
    } catch (e) {
      listError = e instanceof Error ? e.message : 'failed to list replays';
    } finally {
      loading = false;
    }
  });

  function duration(r: ReplaySummary): string {
    if (r.final_tick == null) return 'incomplete';
    const secs = r.final_tick / Math.max(1, r.tick_hz);
    return `${r.final_tick} ticks · ${secs.toFixed(1)}s`;
  }
</script>

<main class="replay-browser">
  <div class="rb-card">
    <header class="rb-head">
      <h1>Replays</h1>
      <button class="topbar-btn" type="button" onclick={() => appMode.set('live')}>
        Back to live
      </button>
    </header>

    <div class="replay-filters"><label>Search teams or match<input class="config-input" type="search" bind:value={search} /></label><label>Outcome<select bind:value={outcome}><option value="all">All outcomes</option><option value="winner">Winner</option><option value="draw">Draw</option><option value="aborted">Aborted</option><option value="incomplete">Incomplete</option><option value="unknown">Unknown</option></select></label></div>
    {#if loading}
      <p class="rb-note">Loading replays…</p>
    {:else if listError}
      <p class="rb-err">{listError}</p>
    {:else if replays.length === 0}
      <p class="rb-note">
        No replays found. Finished matches are recorded to the server's replay directory.
      </p>
    {:else}
      <ul class="rb-list">
        {#each filtered as r (r.replay_id)}
          <li class="rb-item">
            <div class="rb-item-main">
              <div class="rb-bots">{r.bots.join('  vs  ') || '(no bots)'}</div>
              <div class="rb-meta">
                {#if r.winner_name}
                  <span class="rb-winner">Winner: {r.winner_name}</span>
                {:else}
                  <span class="rb-draw">{r.outcome ?? (r.final_tick == null ? 'Incomplete' : 'Unknown')}</span>
                {/if}
                <span>·</span>
                <span>{duration(r)}</span>
                <span>·</span>
                <span class="rb-id">{r.replay_id}</span>
              </div>
            </div>
            <button
              class="pm-start"
              type="button"
              disabled={$replayLoading}
              onclick={() => openReplay(r.replay_id)}
            >
              Start replay
            </button>
          </li>
        {:else}<li class="rb-note">No matching replays.</li>{/each}
      </ul>
    {/if}

    {#if $replayError}
      <p class="rb-err">{$replayError}</p>
    {/if}
  </div>
</main>
