<script lang="ts">
  import { mergeConfig } from '../lib/config';
  import { download } from '../lib/presentation';
  import { room, configSchema, applyConfig, configDraft, roomError } from '../stores/admin';
  import type { SimConfig } from '../types/protocol';
  let busy = $state(false);
  let message = $state<string | null>(null);
  let error = $state<string | null>(null);
  const values = $derived($configDraft ?? $room?.config ?? {});
  const locked = $derived(busy || $room?.state !== 'lobby' || !$room?.capabilities?.manage_match);
  const changes = $derived($configSchema.filter(f => values[f.key] !== $room?.config[f.key]));
  const groups = $derived([...new Set($configSchema.map(f => f.group))]);
  const activePreset = $derived($configSchema.every(f => $room?.config[f.key] === f.default) ? 'Standard' : 'Custom');
  function setDraft(next: SimConfig) {
    configDraft.set(JSON.stringify(next) === JSON.stringify($room?.config) ? null : next);
    message = null; error = null;
  }
  function change(key: string, event: Event) {
    setDraft(mergeConfig(values, { [key]: (event.currentTarget as HTMLInputElement).valueAsNumber }));
  }
  function discard() { configDraft.set(null); error = null; message = 'Draft discarded.'; }
  function defaults() {
    setDraft(mergeConfig($room?.config ?? {}, Object.fromEntries($configSchema.map(f => [f.key, f.default]))));
  }
  async function apply() {
    if (locked || !$configDraft) return;
    const payload = mergeConfig(values, {});
    for (const f of $configSchema) {
      const v = payload[f.key];
      if (typeof v !== 'number' || !Number.isFinite(v) || v < f.min || v > f.max || (f.integer && !Number.isInteger(v))) {
        error = `${f.label}: enter ${f.integer ? 'a whole number' : 'a number'} from ${f.min} to ${f.max}.`; return;
      }
    }
    busy = true; error = null; message = null;
    try { await applyConfig(payload); configDraft.set(null); message = 'Rules applied. Waiting for teams to acknowledge the updated rules.'; }
    catch (e) { error = e instanceof Error ? e.message : 'Could not apply rules'; }
    finally { busy = false; }
  }
  async function importRules(e: Event) {
    const file = (e.currentTarget as HTMLInputElement).files?.[0];
    if (!file || locked) return;
    try {
      if (file.size > 65536) throw new Error('Rules file must be smaller than 64 KiB.');
      const data = JSON.parse(await file.text());
      if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('Expected a rules object.');
      const allowed = new Set([...$configSchema.map(f => f.key), 'powerups']);
      if (Object.keys(data).some(k => !allowed.has(k))) throw new Error('Rules file contains unknown settings.');
      setDraft({ ...$room?.config, ...data, ...(data.powerups ? { powerups: { ...data.powerups } } : {}) });
    } catch (e) { error = e instanceof Error ? e.message : 'Could not import rules'; }
  }
</script>
<div class="config-form">
  <p class="pm-sub">Active preset: <strong>{activePreset}</strong> · rules {$room?.config_hash?.slice(0, 10) ?? '—'}</p>
  <p class="config-note">Applying changes makes every team acknowledge the updated rules before starting. Rules are frozen during a match.</p>
  {#if !$configSchema.length}<p role="status">{$roomError ? 'Settings unavailable. Reconnecting…' : 'Loading settings…'}</p>{/if}
  {#if $configDraft}
    <div class="draft-summary"><strong>{changes.length || 'Imported'} unapplied change{changes.length === 1 ? '' : 's'}</strong>
      <ul>{#each changes as f (f.key)}<li>{f.label}: {$room?.config[f.key]} → {values[f.key]}</li>{/each}</ul>
    </div>
  {/if}
  <div class="config-actions">
    <button class="topbar-btn" disabled={locked} onclick={defaults}>Standard preset</button>
    <button class="topbar-btn" onclick={() => download('naval-rules.json', JSON.stringify(values, null, 2))}>Export rules</button>
    <label class="topbar-btn file-label">Import rules<input type="file" accept=".json,application/json" disabled={locked} onchange={importRules} /></label>
  </div>
  {#each groups as group (group)}
    <details class="config-group"><summary>{group}</summary>
      {#each $configSchema.filter(f => f.group === group) as f (f.key)}
        <label class="config-row"><span class="config-label">{f.label}<small>{f.min}–{f.max}{f.key.endsWith('_ticks') && $room?.tick_hz ? ` ticks · ${$room.tick_hz}/s` : ''}</small></span>
          <input class="config-input" type="number" min={f.min} max={f.max} step={f.integer ? 1 : 'any'} disabled={locked} value={typeof values[f.key] === 'number' ? values[f.key] as number : ''} oninput={e => change(f.key, e)} />
        </label>
      {/each}
    </details>
  {/each}
  {#if $configDraft}<div class="config-actions sticky-save"><button class="pm-start" disabled={locked} onclick={apply}>{busy ? 'Applying…' : 'Apply rules'}</button><button class="topbar-btn" disabled={busy} onclick={discard}>Discard draft</button></div>{/if}
  {#if message}<p class="config-msg" role="status">{message}</p>{/if}
  {#if error}<p class="config-err" role="alert">{error}</p>{/if}
</div>
