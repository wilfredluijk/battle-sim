<script lang="ts">
  import { replayData, replayTick, replayPlaying, replayPerspective, replayPerspectiveData, seekTo, selectPerspective } from '../stores/replay';
  import { training, saveTraining, reloadTraining } from '../stores/admin';
  import { eventText, clockText } from '../lib/presentation';
  import { chipsForShip } from '../lib/powerupHud';
  import type { Debrief } from '../types/protocol';
  let { close }: { close: () => void } = $props();
  let draft = $state<Debrief>({ notes: '', bookmarks: [] });
  let loadedId = $state(''); let base = $state(''); let revision = $state(0);
  let label = $state(''); let error = $state(''); let notice = $state(''); let pending = $state(false);
  let eventLimit = $state(50);
  const id = $derived($replayData?.header.replay_id ?? '');
  const dirty = $derived(JSON.stringify(draft) !== base);
  function loadSaved() {
    if (!$training || !id) return;
    draft = structuredClone($training.debriefs[id] ?? { notes: '', bookmarks: [] });
    base = JSON.stringify(draft); revision = $training.revision; loadedId = id;
    error = ''; notice = '';
  }
  $effect(() => { if ($training && id !== loadedId) loadSaved(); });
  const bot = $derived($replayData?.header.bots.find(b => b.bot_id === $replayPerspective));
  const ownShip = $derived($replayData?.frames[$replayTick]?.ships.find(s => s.id === bot?.ship_id));
  const contacts = $derived($replayPerspectiveData?.frames.find(f => f.tick === $replayTick)?.contacts);
  const names = $derived(new Map($replayData?.header.bots.map(b => [b.ship_id, b.name]) ?? []));
  const events = $derived(($replayData?.frames ?? []).flatMap(f => f.events.filter(e => e.type !== 'shell_splash' && (!bot || ('ship_id' in e && e.ship_id === bot.ship_id))).map(e => ({ tick: f.tick, text: eventText(e, names) }))));
  const hz = $derived($replayData?.header.tick_hz ?? 1);
  function jump(tick: number) { replayPlaying.set(false); seekTo(tick); }
  function bookmark() {
    if (draft.bookmarks.length >= 100) { error = 'This replay already has 100 bookmarks.'; return; }
    draft.bookmarks = [...draft.bookmarks, { tick: $replayTick, label: label.trim() || `Discuss ${clockText($replayTick / hz)}` }].sort((a,b) => a.tick - b.tick);
    label = ''; notice = '';
  }
  async function save() {
    if (pending) return;
    pending = true; error = ''; notice = '';
    const saved = JSON.stringify(draft);
    const unchangedOnServer = JSON.stringify($training?.debriefs[id] ?? { notes: '', bookmarks: [] }) === base;
    try { await saveTraining({ action: 'debrief', replay_id: id, debrief: JSON.parse(saved) }, unchangedOnServer ? $training?.revision : revision); base = saved; revision = $training!.revision; notice = 'Debrief saved on the server.'; }
    catch(e) { error = e instanceof Error ? e.message : 'Could not save debrief'; }
    finally { pending = false; }
  }
  async function reload() {
    try { await reloadTraining(); loadSaved(); } catch(e) { error = e instanceof Error ? e.message : 'Could not reload'; }
  }
</script>
<aside class="debrief-notes" aria-label="Replay debrief guide">
  <header class="pm-head"><h2>Guided debrief</h2><button class="topbar-btn" onclick={close}>Close guide</button></header>
  <p class="pm-sub">1. Choose a team. 2. Compare its contacts with ground truth. 3. Step through the outcome and record what to change.</p>
  <label>Focus team <select value={$replayPerspective} onchange={e => { eventLimit = 50; void selectPerspective(e.currentTarget.value); }}><option value="overall">All teams</option>{#each $replayData?.header.bots ?? [] as b (b.bot_id)}<option value={b.bot_id}>{b.name}</option>{/each}</select></label>
  {#if ownShip}<section><h3>{ownShip.bot_name} · tick {$replayTick}</h3><dl class="rules-summary"><dt>Hull / ammo</dt><dd>{ownShip.hp} HP / {ownShip.ammo}</dd><dt>Speed</dt><dd>{ownShip.speed.toFixed(1)} u/s</dd><dt>Controls</dt><dd>Throttle {ownShip.throttle.toFixed(2)} · Rudder {ownShip.rudder.toFixed(2)}</dd><dt>Sensor</dt><dd>{ownShip.sensor_mode}</dd><dt>Contacts</dt><dd>{contacts?.length ?? 'Loading…'}</dd></dl>{#each chipsForShip(ownShip) as chip (chip.id)}<p class="config-note">{chip.label}: {chip.state}{chip.state === 'active' ? ` · ${(chip.activeTicksLeft / hz).toFixed(1)}s left` : ''}</p>{/each}</section>{/if}
  <details open><summary>Events · {events.length}</summary><ul class="guide-events">{#each events.slice(0,eventLimit) as event, i (i)}<li><button class="topbar-btn" onclick={() => jump(event.tick)}>{clockText(event.tick / hz)} · {event.text}</button></li>{/each}</ul>{#if events.length > eventLimit}<button class="topbar-btn" onclick={() => eventLimit += 50}>Show 50 more</button>{/if}</details>
  <h3>Bookmarks {dirty ? '· unsaved changes' : ''}</h3>
  <label>Bookmark label <input maxlength="120" placeholder="What should we discuss?" bind:value={label} /></label><button class="topbar-btn" disabled={!loadedId || pending} onclick={bookmark}>Bookmark current frame</button>
  <ul class="saved-bookmarks">{#each draft.bookmarks as mark, i (i)}<li><button class="topbar-btn" onclick={() => jump(mark.tick)}>{clockText(mark.tick / hz)} · {mark.label}</button><button class="topbar-btn" disabled={pending} aria-label={`Remove bookmark ${mark.label}`} onclick={() => draft.bookmarks = draft.bookmarks.filter((_, j) => j !== i)}>×</button></li>{/each}</ul>
  <label>Discussion notes <textarea rows="5" maxlength="8000" disabled={pending || !loadedId} bind:value={draft.notes} placeholder="What did the bot observe? Why did it act? What will the team try next?"></textarea></label>
  <p class="config-note">Save to keep notes and bookmarks across browsers and server restarts.</p>
  {#if revision !== $training?.revision && loadedId}<p class="config-note">History changed since this draft loaded. Saving checks for conflicts; reload discards this draft.</p>{/if}
  <div class="session-actions"><button class="topbar-btn primary" disabled={!dirty || pending || !loadedId} onclick={save}>{pending ? 'Saving…' : 'Save debrief'}</button><button class="topbar-btn" disabled={pending} onclick={reload}>Reload saved notes</button></div>
  {#if error}<p class="config-err" role="alert">{error}</p>{/if}{#if notice}<p role="status">{notice}</p>{/if}
</aside>
