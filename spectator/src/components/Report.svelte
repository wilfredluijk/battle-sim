<!-- Post-battle report screen. Shown after a match finishes: outcome banner, duration,
     and a per-bot statistics table. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { report, room, adminToken, showReport, resetMatch } from '../stores/admin';
  import { appMode } from '../stores';
  import { openReplay, replayLoading, replayError } from '../stores/replay';
  import { download } from '../lib/presentation';
  import { colorFor } from '../lib/palette';

  onMount(() => replayError.set(null));
  const r = $derived($report);
  const isAdmin = $derived($adminToken != null);
  const stillEnded = $derived($room?.state === 'ended');

  let resetting = $state(false);
  let error = $state<string | null>(null);

  function dismiss(): void {
    showReport.set(false);
    appMode.set('lobby');
  }

  async function handleReset(): Promise<void> {
    if (resetting) return;
    resetting = true;
    error = null;
    try { await resetMatch(); dismiss(); }
    catch (e) { error = e instanceof Error ? e.message : 'Could not prepare the lobby'; }
    finally { resetting = false; }
  }
  function exportCsv() {
    if (!r) return;
    const cell = (v: unknown) => '"' + String(v).replace(/^[=+@\-]/, "'$&").replace(/"/g, '""') + '"';
    const rows = [['Team', 'Kills', 'Shots', 'Hit events', 'Hits per shot', 'Damage dealt', 'Damage taken', 'Final HP', 'Status'],
      ...r.bots.map(b => [b.name, b.kills, b.shots_fired, b.hits_landed, b.accuracy, b.damage_dealt, b.damage_taken, b.final_hp, b.forfeited ? 'forfeited' : b.survived ? 'survived' : 'destroyed'])];
    download('match-results.csv', rows.map(row => row.map(cell).join(',')).join('\r\n'), 'text/csv');
  }

</script>

<main class="report">
  {#if r}
    <section class="report-card">
      <h1>Match report</h1>

      <div class="report-outcome outcome-{r.outcome}">
        {#if r.outcome === 'winner'}
          {r.winner_name ?? r.winner ?? 'Unknown'} wins
        {:else if r.outcome === 'draw'}
          Draw
        {:else}
          Match aborted
        {/if}
      </div>

      <p class="report-meta">
        Duration {r.duration_ticks} ticks ({r.duration_seconds.toFixed(1)}s)
        {#if r.replay_id}
          · Replay <code>{r.replay_id}</code>
        {/if}
      </p>

      {#if r.end_reason}<p class="report-meta">End reason: {r.end_reason.replace(/_/g, ' ')}</p>{/if}
      <p class="config-note">Hits per shot counts hit events, including multiple targets hit by one shot. This is a training summary; row order is not a competition ranking.</p>
      <div class="table-scroll"><table class="report-table">
        <thead>
          <tr>
            <th class="ra-left">Bot</th>
            <th>Kills</th>
            <th>Shots</th>
            <th>Hits</th>
            <th>Hits per shot</th>
            <th>Dmg dealt</th>
            <th>Dmg taken</th>
            <th>Final HP</th>
            <th class="ra-left">Result</th>
          </tr>
        </thead>
        <tbody>
          {#each r.bots as b (b.bot_id)}
            <tr class:winner-row={r.winner != null && b.bot_id === r.winner}>
              <td class="ra-left">
                <span class="bot-swatch" style="background: {colorFor(b.name)};"></span>
                {b.name}
              </td>
              <td>{b.kills}</td>
              <td>{b.shots_fired}</td>
              <td>{b.hits_landed}</td>
              <td>{b.shots_fired ? b.accuracy.toFixed(2) : '—'}</td>
              <td><meter min="0" max={Math.max(1, ...r.bots.map(bot => bot.damage_dealt))} value={b.damage_dealt} aria-label={`${b.name} damage dealt`}></meter> {b.damage_dealt}</td>
              <td>{b.damage_taken}</td>
              <td>{b.final_hp}</td>
              <td class="ra-left {b.survived ? 'res-survived' : 'res-destroyed'}">
                {b.forfeited ? 'forfeited' : b.survived ? 'survived' : 'destroyed'}
              </td>
            </tr>
          {/each}
        </tbody>
      </table></div>

      {#if error}<p class="config-err" role="alert">{error}</p>{/if}
      {#if $replayError}<p class="config-err" role="alert">{$replayError}</p>{/if}
      <div class="report-actions">
        <button class="topbar-btn" type="button" onclick={dismiss}>Back to lobby</button>
        {#if r?.replay_id}<button class="pm-start" disabled={$replayLoading} onclick={() => r?.replay_id && openReplay(r.replay_id)}>{$replayLoading ? 'Loading replay…' : 'Watch replay'}</button>{/if}
        {#if r}<button class="topbar-btn" onclick={exportCsv}>Export CSV</button><button class="topbar-btn" onclick={() => download('match-results.json', JSON.stringify(r, null, 2))}>Export JSON</button>{/if}
        {#if isAdmin && stillEnded}
          <button
            class="topbar-btn"
            type="button"
            disabled={resetting}
            onclick={handleReset}
            title="Skip the post-game pause and return to the lobby now">
            Reset now
          </button>
        {/if}
      </div>
    </section>
  {:else}
    <section class="report-card">
      <p class="pm-sub">No match report available.</p>
      {#if error}<p class="config-err" role="alert">{error}</p>{/if}
      {#if $replayError}<p class="config-err" role="alert">{$replayError}</p>{/if}
      <div class="report-actions">
        <button class="topbar-btn" type="button" onclick={dismiss}>Back to lobby</button>
      </div>
    </section>
  {/if}
</main>
