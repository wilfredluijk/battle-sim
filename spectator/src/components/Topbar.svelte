<script lang="ts">
  import { connection, view } from '../stores';

  function toggleView(): void {
    view.update((v) => (v === 'split' ? 'full' : 'split'));
  }

  const statusClass = $derived($connection.connected ? 'status-connected' : 'status-disconnected');
  const buttonLabel = $derived($view === 'full' ? 'Split view' : 'Fit battlefield');
  const pressed = $derived($view === 'full' ? 'true' : 'false');
</script>

<nav class="topbar">
  <span class="topbar-title">Naval Battle</span>
  <div id="status" class="status {statusClass}">{$connection.message}</div>
  <span class="topbar-spacer"></span>
  <div class="topbar-controls">
    <button
      id="view-toggle"
      class="topbar-btn"
      type="button"
      aria-pressed={pressed}
      title="Hide the sidebar and let the battlefield fill the window"
      onclick={toggleView}
    >
      {buttonLabel}
    </button>
  </div>
</nav>
