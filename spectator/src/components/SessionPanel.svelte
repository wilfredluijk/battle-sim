<script lang="ts">
  import { room, training, trainingError, reloadTraining, saveTraining, selectedReport, showReport } from '../stores/admin';
  import { appMode } from '../stores';
  import { openReplay, replayError } from '../stores/replay';
  import { standings, standingsCsv } from '../lib/standings';
  import { download } from '../lib/presentation';
  import type { TrainingAction, TrainingSession } from '../types/protocol';
  let selected = $state<string | null>(null);
  const session = $derived($training?.sessions.find(s => s.id === (selected ?? $training.active_session)));
  const rows = $derived(session ? standings(session) : []);
  const editable = $derived($room?.state === 'lobby' && !!$room.capabilities?.manage_match);
  let editing = $state(false); let creating = $state(false); let pending = $state(false);
  let name = $state(''); let teams = $state(''); let nextRound = $state('');
  let win = $state(3); let draw = $state(1); let loss = $state(0);
  let revision = $state(0); let error = $state(''); let notice = $state('');
  function edit(s?: TrainingSession) {
    creating = !s; editing = true; revision = $training?.revision ?? 0;
    name = s?.name ?? ''; teams = (s?.expected_teams ?? $room?.expected_teams ?? []).join('\n');
    nextRound = s?.next_round ?? 'Round 1'; win = s?.scoring.win ?? 3; draw = s?.scoring.draw ?? 1; loss = s?.scoring.loss ?? 0;
    error = ''; notice = '';
  }
  async function act(action: TrainingAction, expected = $training?.revision) {
    pending = true; error = ''; notice = '';
    try { await saveTraining(action, expected); editing = false; selected = $training?.active_session ?? null; notice = 'Session saved.'; }
    catch (e) { error = e instanceof Error ? e.message : 'Could not save session'; }
    finally { pending = false; }
  }
  function save() {
    const settings = { name: name.trim(), expected_teams: teams.split('\n').map(t => t.trim()).filter(Boolean), scoring: { win, draw, loss } };
    void act(creating ? { action: 'create', ...settings } : { action: 'configure', ...settings, next_round: nextRound.trim() }, revision);
  }
  async function reload() { try { await reloadTraining(); editing = false; error = ''; } catch(e) { error = e instanceof Error ? e.message : 'Could not reload'; } }
</script>
<main class="session-page">
  <section class="pm-panel">
    <header class="pm-head"><h1>Training sessions</h1><button class="topbar-btn primary" disabled={!editable || pending || !$training} onclick={() => edit()}>New session</button></header>
    <p class="pm-sub">Keep named rounds, results and training points across server restarts. Expected names match the teams’ connection identities; credentials stay separate.</p>
    {#if $trainingError}<p class="config-err" role="alert">{$trainingError}</p>{/if}
    {#if $training?.storage_error || $room?.training_error}<p class="config-err" role="alert">{$training?.storage_error ?? $room?.training_error} Unsaved results remain available while this server stays running.</p>{/if}
    <div class="session-actions">
      <label>Session <select value={session?.id ?? ''} onchange={e => { selected = e.currentTarget.value; editing = false; }}><option value="" disabled>Select a session</option>{#each $training?.sessions ?? [] as s (s.id)}<option value={s.id}>{s.name}{s.id === $training?.active_session ? ' · active' : ''}</option>{/each}</select></label>
      <button class="topbar-btn" disabled={pending} onclick={reload}>Reload history</button>
      {#if $training}<button class="topbar-btn" onclick={() => download('training-history.json', JSON.stringify($training, null, 2), 'application/json')}>Export all history</button>{/if}
    </div>
    {#if editing}
      <form class="session-form" onsubmit={e => { e.preventDefault(); save(); }}>
        <h2>{creating ? 'Create a training session' : 'Session settings'}</h2>
        <label>Session name <input required maxlength="120" bind:value={name} /></label>
        <label>Expected teams · one identity per line <textarea rows="5" bind:value={teams}></textarea></label>
        {#if !creating}<label>Next round name <input required maxlength="120" bind:value={nextRound} /></label>{/if}
        <fieldset disabled={!creating && !!session?.rounds.length}><legend>Training points · locked after the first round</legend><div class="session-actions">
          <label>Win <input type="number" min="0" max="1000" step="1" required bind:value={win} /></label>
          <label>Draw <input type="number" min="0" max="1000" step="1" required bind:value={draw} /></label>
          <label>Loss <input type="number" min="0" max="1000" step="1" required bind:value={loss} /></label>
        </div></fieldset>
        <p class="config-note">Forfeits earn 0. Aborted and interrupted rounds are excluded. Every non-forfeiting participant in a draw receives draw points. Equal points share a place.</p>
        {#if revision !== $training?.revision}<p class="config-err">History changed while editing. Reload history before saving.</p>{/if}
        <div class="session-actions"><button class="topbar-btn primary" disabled={!editable || pending || revision !== $training?.revision}>{pending ? 'Saving…' : 'Save session'}</button><button type="button" class="topbar-btn" disabled={pending} onclick={() => editing = false}>Cancel</button></div>
      </form>
    {/if}
    {#if error}<p class="config-err" role="alert">{error}</p>{/if}{#if notice}<p role="status">{notice}</p>{/if}
    {#if !editable}<p class="config-note">Session settings can be changed in the lobby.</p>{/if}
    {#if session}
      <header class="pm-head"><div><h2>{session.name}</h2><p class="pm-sub">Next: {session.next_round} · {session.expected_teams.length} expected teams · {session.rounds.length} rounds recorded</p></div>
        {#if session.id === $training?.active_session}<div class="session-actions"><button class="topbar-btn" disabled={!editable || pending} onclick={() => edit(session)}>Edit session / name next round</button><button class="topbar-btn" disabled={!editable || pending} onclick={() => act({ action: 'activate', session_id: null })}>Pause session</button></div>
        {:else}<button class="topbar-btn" disabled={!editable || pending} onclick={() => act({ action: 'activate', session_id: session.id })}>Use for next rounds</button>{/if}
      </header>
      <h2>Training standings</h2>
      <p class="pm-sub">Win {session.scoring.win} · Draw {session.scoring.draw} · Loss {session.scoring.loss} · Forfeit 0. Aborted/interrupted rounds excluded. Equal points share a place. These are training points.</p>
      <div class="table-scroll"><table class="session-table"><thead><tr><th>Place</th><th>Team</th><th>Points</th><th>Played</th><th>Wins</th><th>Draws</th><th>Losses</th><th>Forfeits</th><th>Damage</th></tr></thead><tbody>{#each rows as row (row.name)}<tr><td>{row.place}</td><th>{row.name}</th><td><strong>{row.points}</strong></td><td>{row.played}</td><td>{row.wins}</td><td>{row.draws}</td><td>{row.losses}</td><td>{row.forfeits}</td><td>{row.damage}</td></tr>{/each}</tbody></table></div>
      <div class="session-actions"><button class="topbar-btn" onclick={() => download(`session-${session.id}.csv`, standingsCsv(session), 'text/csv')}>Export standings CSV</button><button class="topbar-btn" onclick={() => download(`session-${session.id}.json`, JSON.stringify({ ...session, standings: rows }, null, 2), 'application/json')}>Export session JSON</button></div>
      <h2>Round history</h2>
      {#if !session.rounds.length}<p class="pm-sub">Start a match from the lobby to record the first round.</p>{/if}
      <ul class="round-history">{#each [...session.rounds].reverse() as round (round.match_id)}<li>
        <div><strong>{round.name}</strong><small>{new Date(round.started_at * 1000).toLocaleString()} · {round.teams.join(', ')}</small><small>{round.report ? `${round.report.outcome} · ${round.report.winner_name ?? round.report.end_reason ?? ''} · ${round.report.duration_seconds.toFixed(1)}s` : round.status === 'interrupted' ? 'Interrupted by server restart · no recorded result' : 'Running'}</small></div>
        {#if round.report}<button class="topbar-btn" onclick={() => { selectedReport.set(round.report); showReport.set(true); appMode.set('results'); }}>View result</button>{/if}
        {#if round.report?.replay_id}<button class="topbar-btn" onclick={() => openReplay(round.report!.replay_id!)}>Debrief replay</button>{/if}
      </li>{/each}</ul>
      {#if $replayError}<p class="config-err" role="alert">{$replayError}</p>{/if}
    {:else if $training}<p class="pm-sub">Create or select a session to group rounds. Matches outside a session still have their own report and replay.</p>{:else}<p class="pm-sub">Loading saved sessions…</p>{/if}
  </section>
</main>
