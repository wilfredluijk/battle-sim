<script lang="ts">
  import { appMode, connection, view } from '../stores';
  import { room, adminToken, abortMatch } from '../stores/admin';
  import LoginBox from './LoginBox.svelte';

  let abortBusy = $state(false);

  const statusClass = $derived(
    $connection.connected ? 'status-connected' : 'status-disconnected',
  );
  const roomState = $derived($room?.state ?? null);
  const isRunning = $derived(roomState === 'running');
  const buttonLabel = $derived($view === 'full' ? 'Split view' : 'Fit battlefield');

  function toggleView(): void {
    view.update((v) => (v === 'split' ? 'full' : 'split'));
  }

  async function handleAbort(): Promise<void> {
    if (abortBusy) return;
    abortBusy = true;
    try {
      await abortMatch();
    } catch {
      // The next room poll reflects the real state; surfacing a toast here is overkill.
    } finally {
      abortBusy = false;
    }
  }
</script>

<nav class="topbar">
  <span class="topbar-title">Naval Battle</span>
  <div id="status" class="status {statusClass}">{$connection.message}</div>
  {#if roomState}
    <span class="admin-badge admin-badge-{roomState}">{roomState}</span>
  {/if}
  <span class="topbar-spacer"></span>
  <LoginBox />
  <div class="topbar-controls">
    {#if $appMode === 'live'}
      <button
        class="topbar-btn"
        type="button"
        title="Browse and watch recorded matches"
        onclick={() => appMode.set('replay-browser')}>Replays</button>
    {/if}
    {#if isRunning && $adminToken}
      <button
        class="topbar-btn admin-disconnect"
        type="button"
        disabled={abortBusy}
        onclick={handleAbort}
        title="Force-end the running match (no winner)">Abort match</button>
    {/if}
    {#if isRunning}
      <button
        id="view-toggle"
        class="topbar-btn"
        type="button"
        aria-pressed={$view === 'full' ? 'true' : 'false'}
        title="Hide the sidebar and let the battlefield fill the window"
        onclick={toggleView}
      >
        {buttonLabel}
      </button>
    {/if}
  </div>
</nav>
