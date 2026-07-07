<!-- Monte Carlo batch runner.

     Three states share one panel:
     - Setup     — when no run is active. Form for n_matches, mc_seed, variance mode, etc.
     - Running   — progress bar, live win tally, last-N results, Stop button.
     - Completed — final results table with Wilson 95% CIs, replay download links.
-->
<script lang="ts">
  import { appMode } from '../stores';
  import {
    adminToken,
    mcStatus,
    mcError,
    room,
    roomError,
    startMonteCarlo as startRun,
    stopMonteCarlo as stopRun,
  } from '../stores/admin';
  import { colorFor } from '../lib/palette';
  import { wilsonInterval } from '../lib/mcConfidence';
  import { mcPanelPhase } from '../lib/mcPanelPhase';
  import type { McStartRequest, VarianceMode } from '../types/protocol';

  const isAdmin = $derived($adminToken != null);
  const bots = $derived($room?.bots ?? []);
  const readyBots = $derived(bots.filter((b) => b.ready));
  const allReady = $derived(bots.length >= 2 && bots.every((b) => b.ready));

  // Form state — populated with sensible defaults.
  let nMatches = $state(100);
  let mcSeed = $state(42);
  let varianceMode = $state<VarianceMode>('shuffled');
  let perTickTimeoutMs = $state(1000);
  let spectatorThrottle = $state(5);
  let starting = $state(false);
  let stopping = $state(false);
  // Local override: "New run" sets this so the setup form reappears even though the server
  // keeps reporting the last completed run's status. Cleared once a run is actually running.
  let showSetup = $state(false);

  // Derived: which state the panel is in.
  const phase = $derived(mcPanelPhase($mcStatus, showSetup));

  async function handleStart(): Promise<void> {
    if (starting) return;
    starting = true;
    try {
      const cfg: McStartRequest = {
        n_matches: Math.max(1, Math.floor(nMatches)),
        mc_seed: Math.floor(mcSeed),
        variance_mode: varianceMode,
        per_tick_timeout_ms: Math.max(1, Math.floor(perTickTimeoutMs)),
        spectator_throttle: Math.max(0, Math.floor(spectatorThrottle)),
      };
      await startRun(cfg);
      // A run is starting; drop the local override so we follow live status again.
      showSetup = false;
    } catch {
      /* error surfaced in mcError */
    } finally {
      starting = false;
    }
  }

  async function handleStop(forceAbort = false): Promise<void> {
    if (stopping) return;
    stopping = true;
    try {
      await stopRun(forceAbort);
    } catch {
      /* error surfaced in mcError */
    } finally {
      stopping = false;
    }
  }

  function randomizeSeed(): void {
    // Use plain JS rand for the seed value — it's an input, not part of the deterministic
    // simulation. The server's RNG is still seeded with this number.
    mcSeed = Math.floor(Math.random() * 2 ** 31);
  }

  // ----- Live-progress derivations -----------------------------------------

  const elapsedSecs = $derived.by(() => {
    const status = $mcStatus;
    if (!status) return 0;
    const nowSec = Math.floor(Date.now() / 1000);
    const start = status.started_at_unix || nowSec;
    const end = status.finished_at_unix ?? nowSec;
    return Math.max(0, end - start);
  });

  const avgMatchSecs = $derived.by(() => {
    const status = $mcStatus;
    if (!status || status.completed === 0) return 0;
    return elapsedSecs / status.completed;
  });

  const etaSecs = $derived.by(() => {
    const status = $mcStatus;
    if (!status || !status.running || status.completed === 0) return null;
    return Math.round(avgMatchSecs * (status.total - status.completed));
  });

  const progressPct = $derived.by(() => {
    const status = $mcStatus;
    if (!status || status.total === 0) return 0;
    return Math.min(100, (status.completed / status.total) * 100);
  });

  function formatSeconds(s: number | null): string {
    if (s == null || !Number.isFinite(s)) return '–';
    if (s < 60) return `${Math.round(s)}s`;
    const m = Math.floor(s / 60);
    const sec = Math.round(s - m * 60);
    return `${m}m ${sec.toString().padStart(2, '0')}s`;
  }

  function formatPct(x: number): string {
    return `${(x * 100).toFixed(1)}%`;
  }

  // Bot leaderboard rows — derived once per status push so the template stays tidy.
  interface LeaderboardRow {
    botId: string;
    name: string;
    wins: number;
    rate: number;
    ciLower: number;
    ciUpper: number;
  }
  const leaderboard = $derived.by<LeaderboardRow[]>(() => {
    const status = $mcStatus;
    if (!status) return [];
    const total = Math.max(1, status.completed);
    const rows: LeaderboardRow[] = Object.entries(status.wins).map(([botId, w]) => {
      const ci = wilsonInterval(w, total);
      return {
        botId,
        name: status.bot_names[botId] ?? botId,
        wins: w,
        rate: ci.point,
        ciLower: ci.lower,
        ciUpper: ci.upper,
      };
    });
    // Include bots with zero wins so the table doesn't hide losing entries.
    for (const [botId, name] of Object.entries(status.bot_names)) {
      if (!rows.find((r) => r.botId === botId)) {
        const ci = wilsonInterval(0, total);
        rows.push({ botId, name, wins: 0, rate: 0, ciLower: ci.lower, ciUpper: ci.upper });
      }
    }
    rows.sort((a, b) => b.wins - a.wins);
    return rows;
  });
</script>

<main class="mc-panel">
  <div class="mc-card">
    <header class="mc-head">
      <h1>Monte Carlo</h1>
      <button
        class="topbar-btn"
        type="button"
        onclick={() => appMode.set('live')}>Back to live</button>
    </header>

    {#if !$room}
      <p class="rb-note">
        {$roomError
          ? `Cannot reach the server: ${$roomError}`
          : 'Connecting to the server…'}
      </p>
    {:else if phase === 'setup'}
      <!-- Setup form ----------------------------------------------------- -->
      <section class="mc-section">
        <h2>Roster</h2>
        <p class="mc-sub">
          {bots.length}
          {bots.length === 1 ? 'bot' : 'bots'} connected · {readyBots.length} ready
        </p>
        <ul class="pm-bots">
          {#each bots as b (b.bot_id)}
            <li class="pm-bot">
              <span class="bot-swatch" style="background: {colorFor(b.name)};"></span>
              <span class="bot-name">{b.name}</span>
              <span class="bot-status {b.ready ? 'pill-good' : 'pill-muted'}">
                {b.ready ? 'ready' : 'not ready'}
              </span>
              <span></span>
            </li>
          {/each}
        </ul>
      </section>

      <section class="mc-section">
        <h2>Run setup</h2>
        <fieldset class="config-group">
          <legend>Batch parameters</legend>
          <label class="config-row">
            <span class="config-label">Number of matches</span>
            <input
              class="config-input"
              type="number"
              min="1"
              max="10000"
              step="1"
              bind:value={nMatches}
              disabled={!isAdmin}
            />
          </label>
          <label class="config-row">
            <span class="config-label">Seed</span>
            <span class="mc-seed-input">
              <input
                class="config-input"
                type="number"
                min="0"
                step="1"
                bind:value={mcSeed}
                disabled={!isAdmin}
              />
              <button
                class="topbar-btn"
                type="button"
                onclick={randomizeSeed}
                disabled={!isAdmin}
                title="Pick a random seed">🎲</button>
            </span>
          </label>
          <label class="config-row">
            <span class="config-label">Starting positions</span>
            <select
              class="config-input"
              bind:value={varianceMode}
              disabled={!isAdmin}
            >
              <option value="fixed">Fixed (ring, identical layout)</option>
              <option value="rotated">Rotated (random ring rotation)</option>
              <option value="shuffled">Shuffled (rotate + permute slots)</option>
              <option value="random">Random (sampled within map)</option>
            </select>
          </label>
          <label class="config-row">
            <span class="config-label">Per-tick timeout (ms)</span>
            <input
              class="config-input"
              type="number"
              min="1"
              max="300000"
              step="50"
              bind:value={perTickTimeoutMs}
              disabled={!isAdmin}
            />
          </label>
          <label class="config-row">
            <span class="config-label">Spectator broadcast</span>
            <select
              class="config-input"
              bind:value={spectatorThrottle}
              disabled={!isAdmin}
            >
              <option value={0}>Off (fastest)</option>
              <option value={20}>Every 20 ticks</option>
              <option value={10}>Every 10 ticks</option>
              <option value={5}>Every 5 ticks</option>
              <option value={1}>Every tick (slowest)</option>
            </select>
          </label>
        </fieldset>

        {#if isAdmin}
          <div class="config-actions">
            <button
              class="pm-start"
              type="button"
              onclick={handleStart}
              disabled={starting || !allReady || bots.length < 2}
              title={allReady
                ? 'Start the Monte Carlo batch'
                : 'Need at least two bots, all ready'}>
              {starting ? 'Starting…' : 'Start Monte Carlo run'}
            </button>
          </div>
        {:else}
          <p class="config-note">Log in as admin to start a Monte Carlo run.</p>
        {/if}
        {#if $mcError}<p class="config-err">{$mcError}</p>{/if}
      </section>
    {:else if phase === 'running'}
      <!-- Live progress view -------------------------------------------- -->
      {@const status = $mcStatus!}
      <section class="mc-section">
        <div class="mc-progress-row">
          <div class="mc-progress-bar">
            <div class="mc-progress-fill" style="width: {progressPct}%"></div>
          </div>
          <span class="mc-progress-text">
            {status.completed} / {status.total}
          </span>
        </div>
        <div class="mc-meta">
          <span>Elapsed: <strong>{formatSeconds(elapsedSecs)}</strong></span>
          <span>·</span>
          <span>ETA: <strong>{formatSeconds(etaSecs)}</strong></span>
          <span>·</span>
          <span>Avg/match: <strong>{formatSeconds(avgMatchSecs)}</strong></span>
          <span>·</span>
          <span>Current tick: <strong>{status.current_match_tick}</strong></span>
        </div>
      </section>

      <section class="mc-section">
        <h2>Wins</h2>
        <table class="mc-table">
          <thead>
            <tr><th class="ra-left">Bot</th><th>Wins</th><th>Rate</th><th>95% CI</th></tr>
          </thead>
          <tbody>
            {#each leaderboard as row (row.botId)}
              <tr>
                <td class="ra-left">
                  <span class="bot-swatch" style="background: {colorFor(row.name)};"></span>
                  {row.name}
                </td>
                <td>{row.wins}</td>
                <td>{formatPct(row.rate)}</td>
                <td>{formatPct(row.ciLower)} – {formatPct(row.ciUpper)}</td>
              </tr>
            {/each}
            {#if status.draws > 0}
              <tr>
                <td class="ra-left mc-draw-label">draws</td>
                <td>{status.draws}</td>
                <td>{formatPct(status.draws / Math.max(1, status.completed))}</td>
                <td></td>
              </tr>
            {/if}
          </tbody>
        </table>
      </section>

      <section class="mc-section">
        <h2>Recent matches</h2>
        {#if status.results.length === 0}
          <p class="mc-sub">No matches completed yet.</p>
        {:else}
          <ul class="mc-results">
            {#each [...status.results].reverse() as r (r.match_index)}
              <li class="mc-result-row">
                <span class="mc-result-idx">#{r.match_index}</span>
                <span class="mc-result-winner">
                  {r.winner_name ?? r.winner ?? 'draw'}
                </span>
                <span class="mc-result-duration">{r.duration_ticks} ticks</span>
                {#if r.replay_id}
                  <a class="mc-result-link" href={`/api/replays/${r.replay_id}`} target="_blank">replay</a>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      {#if isAdmin}
        <div class="config-actions">
          <button
            class="topbar-btn admin-disconnect"
            type="button"
            onclick={() => handleStop(false)}
            disabled={stopping}>
            {stopping ? 'Stopping…' : 'Stop after current match'}
          </button>
          <button
            class="topbar-btn admin-disconnect"
            type="button"
            onclick={() => handleStop(true)}
            disabled={stopping}>
            Force stop now
          </button>
        </div>
      {/if}
      {#if $mcError}<p class="config-err">{$mcError}</p>{/if}
    {:else}
      <!-- Completed view ------------------------------------------------ -->
      {@const status = $mcStatus!}
      <section class="mc-section">
        <div class="mc-completed-banner">
          <span>{status.completed} matches</span>
          <span>·</span>
          <span>{formatSeconds(elapsedSecs)}</span>
          <span>·</span>
          <span>
            {status.ended_reason === 'completed' ? 'Completed' : `Ended: ${status.ended_reason ?? 'unknown'}`}
          </span>
        </div>
      </section>

      <section class="mc-section">
        <h2>Final rankings</h2>
        <table class="mc-table">
          <thead>
            <tr><th class="ra-left">Bot</th><th>Wins</th><th>Rate</th><th>95% CI</th></tr>
          </thead>
          <tbody>
            {#each leaderboard as row, i (row.botId)}
              <tr class={i === 0 ? 'winner-row' : ''}>
                <td class="ra-left">
                  <span class="bot-swatch" style="background: {colorFor(row.name)};"></span>
                  {row.name}
                </td>
                <td>{row.wins}</td>
                <td>{formatPct(row.rate)}</td>
                <td>{formatPct(row.ciLower)} – {formatPct(row.ciUpper)}</td>
              </tr>
            {/each}
            {#if status.draws > 0}
              <tr>
                <td class="ra-left mc-draw-label">draws</td>
                <td>{status.draws}</td>
                <td>{formatPct(status.draws / Math.max(1, status.completed))}</td>
                <td></td>
              </tr>
            {/if}
          </tbody>
        </table>
      </section>

      <section class="mc-section">
        <h2>Matches</h2>
        <ul class="mc-results">
          {#each [...status.results].reverse() as r (r.match_index)}
            <li class="mc-result-row">
              <span class="mc-result-idx">#{r.match_index}</span>
              <span class="mc-result-winner">
                {r.winner_name ?? r.winner ?? 'draw'}
              </span>
              <span class="mc-result-duration">{r.duration_ticks} ticks</span>
              {#if r.replay_id}
                <a class="mc-result-link" href={`/api/replays/${r.replay_id}`} target="_blank">replay</a>
              {/if}
            </li>
          {/each}
        </ul>
        {#if status.results.length < status.completed}
          <p class="mc-sub">
            (showing the last {status.results.length} of {status.completed} matches —
            older replays are on disk at <code>/api/replays</code>)
          </p>
        {/if}
      </section>

      {#if isAdmin}
        <div class="config-actions">
          <button
            class="pm-start"
            type="button"
            onclick={() => (showSetup = true)}>
            New run
          </button>
        </div>
      {/if}
    {/if}
  </div>
</main>

<style>
  .mc-panel {
    display: flex;
    justify-content: center;
    overflow-y: auto;
  }
  .mc-card {
    background: var(--panel);
    border: 1px solid var(--panel-border);
    border-radius: 6px;
    padding: 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 18px;
    width: 100%;
    max-width: 920px;
  }
  .mc-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .mc-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .mc-sub {
    margin: 0;
    font-size: 12px;
    color: var(--ink-muted);
  }
  .mc-progress-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .mc-progress-bar {
    flex: 1;
    height: 14px;
    border-radius: 7px;
    background: rgba(255, 255, 255, 0.06);
    overflow: hidden;
  }
  .mc-progress-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.25s linear;
  }
  .mc-progress-text {
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    min-width: 80px;
    text-align: right;
  }
  .mc-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 12px;
    color: var(--ink-muted);
    font-variant-numeric: tabular-nums;
  }
  .mc-meta strong {
    color: var(--ink);
  }
  .mc-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .mc-table th,
  .mc-table td {
    padding: 6px 10px;
    text-align: right;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }
  .mc-table th {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-muted);
  }
  .mc-table .ra-left {
    text-align: left;
  }
  .mc-table tbody tr.winner-row {
    background: rgba(88, 214, 141, 0.08);
  }
  .mc-table .bot-swatch {
    display: inline-block;
    margin-right: 8px;
    vertical-align: middle;
  }
  .mc-results {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 320px;
    overflow-y: auto;
  }
  .mc-result-row {
    display: grid;
    grid-template-columns: 50px 1fr 100px 60px;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.04);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .mc-result-idx {
    color: var(--ink-muted);
  }
  .mc-result-winner {
    font-weight: 600;
  }
  .mc-result-duration {
    color: var(--ink-muted);
    text-align: right;
  }
  .mc-result-link {
    color: var(--accent);
    text-decoration: none;
    text-align: right;
    font-size: 11px;
  }
  .mc-result-link:hover {
    text-decoration: underline;
  }
  .mc-seed-input {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 4px;
  }
  .mc-completed-banner {
    display: flex;
    gap: 8px;
    padding: 10px 12px;
    border-radius: 6px;
    background: rgba(108, 177, 255, 0.08);
    border: 1px solid rgba(108, 177, 255, 0.35);
    color: var(--accent);
    font-size: 13px;
    font-weight: 500;
  }
  .mc-draw-label {
    font-style: italic;
    color: var(--ink-muted);
  }
</style>
