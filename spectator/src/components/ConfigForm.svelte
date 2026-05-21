<!-- Pre-match parameter form. Renders one number input per tunable, grouped by category.
     Admins can edit and PUT the values; everyone else sees them read-only. -->
<script lang="ts">
  import { room, configSchema, adminToken, applyConfig } from '../stores/admin';
  import type { ConfigField, SimConfig } from '../types/protocol';

  const isAdmin = $derived($adminToken != null);

  // Working copy of the form values. Seeded once from the room's active config; further
  // server-side changes are picked up via the Revert button rather than clobbering edits.
  let values = $state<Record<string, number>>({});
  let seeded = $state(false);
  let busy = $state(false);
  let message = $state<string | null>(null);
  let error = $state<string | null>(null);

  function valuesFrom(cfg: SimConfig, schema: ConfigField[]): Record<string, number> {
    const v: Record<string, number> = {};
    for (const f of schema) v[f.key] = cfg[f.key] ?? f.default;
    return v;
  }

  $effect(() => {
    const cfg = $room?.config;
    const schema = $configSchema;
    if (!seeded && cfg && schema.length > 0) {
      values = valuesFrom(cfg, schema);
      seeded = true;
    }
  });

  // Tunables grouped by category, in schema order.
  const groups = $derived.by(() => {
    const m = new Map<string, ConfigField[]>();
    for (const f of $configSchema) {
      const list = m.get(f.group) ?? [];
      list.push(f);
      m.set(f.group, list);
    }
    return [...m.entries()];
  });

  function revert(): void {
    const cfg = $room?.config;
    if (cfg) values = valuesFrom(cfg, $configSchema);
    message = null;
    error = null;
  }

  function loadDefaults(): void {
    const v: Record<string, number> = {};
    for (const f of $configSchema) v[f.key] = f.default;
    values = v;
    message = null;
    error = null;
  }

  async function apply(): Promise<void> {
    if (busy) return;
    message = null;
    error = null;

    // Build the payload, coercing integer fields and rejecting non-numeric entries
    // before they hit the server.
    const payload: SimConfig = {};
    for (const f of $configSchema) {
      const raw = values[f.key];
      if (typeof raw !== 'number' || !Number.isFinite(raw)) {
        error = `${f.label} must be a number`;
        return;
      }
      payload[f.key] = f.integer ? Math.round(raw) : raw;
    }

    busy = true;
    try {
      await applyConfig(payload);
      message = 'Parameters applied.';
    } catch (e) {
      error = e instanceof Error ? e.message : 'failed to apply parameters';
    } finally {
      busy = false;
    }
  }
</script>

<div class="config-form">
  {#if $configSchema.length === 0}
    <p class="config-note">Loading parameters…</p>
  {:else}
    {#each groups as [group, fields] (group)}
      <fieldset class="config-group">
        <legend>{group}</legend>
        {#each fields as f (f.key)}
          <label class="config-row">
            <span class="config-label" title={f.key}>{f.label}</span>
            <input
              class="config-input"
              type="number"
              min={f.min}
              max={f.max}
              step={f.integer ? 1 : 'any'}
              disabled={!isAdmin || busy}
              bind:value={values[f.key]}
            />
          </label>
        {/each}
      </fieldset>
    {/each}

    {#if isAdmin}
      <div class="config-actions">
        <button class="topbar-btn" type="button" onclick={apply} disabled={busy}>
          Apply parameters
        </button>
        <button class="topbar-btn" type="button" onclick={revert} disabled={busy}>
          Revert
        </button>
        <button class="topbar-btn" type="button" onclick={loadDefaults} disabled={busy}>
          Defaults
        </button>
      </div>
      {#if message}<p class="config-msg">{message}</p>{/if}
      {#if error}<p class="config-err">{error}</p>{/if}
    {:else}
      <p class="config-note">Log in as admin to edit parameters.</p>
    {/if}
  {/if}
</div>
