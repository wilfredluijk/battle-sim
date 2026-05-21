<!-- Replay picker: lists the replays on disk and opens one in the viewer. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchReplays } from '../lib/replayApi';
  import { openReplay, replayError, replayLoading } from '../stores/replay';
  import { appMode } from '../stores';
  import type { ReplaySummary } from '../types/protocol';

  let replays = $state<ReplaySummary[]>([]);
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
        {#each replays as r (r.replay_id)}
          <li class="rb-item">
            <div class="rb-item-main">
              <div class="rb-bots">{r.bots.join('  vs  ') || '(no bots)'}</div>
              <div class="rb-meta">
                {#if r.winner_name}
                  <span class="rb-winner">Winner: {r.winner_name}</span>
                {:else}
                  <span class="rb-draw">Draw / aborted / incomplete</span>
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
        {/each}
      </ul>
    {/if}

    {#if $replayError}
      <p class="rb-err">{$replayError}</p>
    {/if}
  </div>
</main>
