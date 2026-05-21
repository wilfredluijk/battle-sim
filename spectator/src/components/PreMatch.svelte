<!-- Pre-match lobby screen. Shown when no match is running: a summary of connected bots
     and the editable balance-parameter form. This is the landing screen. -->
<script lang="ts">
  import {
    room,
    roomError,
    adminToken,
    report,
    showReport,
    startMatch,
    kickBot,
  } from '../stores/admin';
  import { colorFor } from '../lib/palette';
  import ConfigForm from './ConfigForm.svelte';

  const isAdmin = $derived($adminToken != null);
  const bots = $derived($room?.bots ?? []);
  const allReady = $derived(bots.length > 0 && bots.every((b) => b.ready));
  const connected = $derived($room != null);

  let starting = $state(false);
  let kicking = $state<string | null>(null);
  let actionError = $state<string | null>(null);

  async function handleStart(): Promise<void> {
    if (starting) return;
    starting = true;
    actionError = null;
    try {
      await startMatch();
    } catch (e) {
      actionError = e instanceof Error ? e.message : 'failed to start match';
    } finally {
      starting = false;
    }
  }

  async function handleKick(botId: string): Promise<void> {
    kicking = botId;
    actionError = null;
    try {
      await kickBot(botId);
    } catch (e) {
      actionError = e instanceof Error ? e.message : 'failed to kick bot';
    } finally {
      kicking = null;
    }
  }
</script>

<main class="prematch">
  <section class="pm-panel">
    <header class="pm-head">
      <h1>Lobby</h1>
      {#if $report}
        <button
          class="topbar-btn"
          type="button"
          onclick={() => showReport.set(true)}>View last match report</button>
      {/if}
    </header>

    {#if !connected}
      <p class="pm-sub">
        {$roomError ? `Cannot reach the server: ${$roomError}` : 'Connecting to the server…'}
      </p>
    {:else}
      <p class="pm-sub">
        {#if bots.length === 0}
          Waiting for bots to connect…
        {:else}
          {bots.length}
          {bots.length === 1 ? 'bot' : 'bots'} connected ·
          {allReady ? 'all ready' : 'waiting for bots to ready up'}
        {/if}
      </p>

      <ul class="pm-bots">
        {#each bots as b (b.bot_id)}
          <li class="pm-bot">
            <span class="bot-swatch" style="background: {colorFor(b.name)};"></span>
            <span class="bot-name" title="{b.name} ({b.bot_id})">{b.name}</span>
            <span class="bot-status {b.ready ? 'pill-good' : 'pill-muted'}">
              {b.ready ? 'ready' : 'not ready'}
            </span>
            {#if isAdmin}
              <button
                class="bot-kick"
                type="button"
                disabled={kicking === b.bot_id}
                onclick={() => handleKick(b.bot_id)}
                title="Disconnect {b.name}">Kick</button>
            {/if}
          </li>
        {/each}
      </ul>

      {#if isAdmin}
        <button
          class="pm-start"
          type="button"
          disabled={starting || !allReady}
          onclick={handleStart}
          title={allReady
            ? 'Start the match'
            : 'Every connected bot must be ready first'}>
          {starting ? 'Starting…' : 'Start match'}
        </button>
      {:else}
        <p class="config-note">Log in as admin to start the match.</p>
      {/if}
      {#if actionError}<p class="config-err">{actionError}</p>{/if}
    {/if}
  </section>

  <section class="pm-panel">
    <h1>Match parameters</h1>
    <p class="pm-sub">Tunables are frozen when the match starts and recorded in the replay.</p>
    <ConfigForm />
  </section>
</main>
