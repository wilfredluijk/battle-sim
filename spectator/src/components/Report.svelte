<!-- Post-battle report screen. Shown after a match finishes: outcome banner, duration,
     and a per-bot statistics table. -->
<script lang="ts">
  import { report, room, adminToken, showReport, resetMatch } from '../stores/admin';
  import { colorFor } from '../lib/palette';

  const r = $derived($report);
  const isAdmin = $derived($adminToken != null);
  const stillEnded = $derived($room?.state === 'ended');

  let resetting = $state(false);

  function dismiss(): void {
    showReport.set(false);
  }

  async function handleReset(): Promise<void> {
    if (resetting) return;
    resetting = true;
    try {
      await resetMatch();
    } catch {
      // Either way we leave the report screen; the poll reflects the real state.
    } finally {
      resetting = false;
      showReport.set(false);
    }
  }

  function pct(x: number): string {
    return `${Math.round(x * 100)}%`;
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
          Draw — no survivors
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

      <table class="report-table">
        <thead>
          <tr>
            <th class="ra-left">Bot</th>
            <th>Kills</th>
            <th>Shots</th>
            <th>Hits</th>
            <th>Accuracy</th>
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
              <td>{pct(b.accuracy)}</td>
              <td>{b.damage_dealt}</td>
              <td>{b.damage_taken}</td>
              <td>{b.final_hp}</td>
              <td class="ra-left {b.survived ? 'res-survived' : 'res-destroyed'}">
                {b.survived ? 'survived' : 'destroyed'}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>

      <div class="report-actions">
        <button class="topbar-btn" type="button" onclick={dismiss}>Back to lobby</button>
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
      <div class="report-actions">
        <button class="topbar-btn" type="button" onclick={dismiss}>Back to lobby</button>
      </div>
    </section>
  {/if}
</main>
