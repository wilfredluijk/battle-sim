<!-- Inline admin panel mounted in the Topbar. Unauthenticated state shows a token input;
     authenticated state shows the current room status + Start / Abort / Reset / Disconnect
     buttons. Per-bot Kick buttons live in BotCard so they're co-located with the bot row. -->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import {
    adminConn,
    adminRoom,
    adminToken,
    adminSendAbort,
    adminSendReset,
    adminSendStart,
    startAdmin,
    stopAdmin,
  } from '../stores/admin';

  let tokenInput = $state($adminToken ?? '');
  let teardown: (() => void) | null = null;
  let attempted = $state(false);

  // Auto-connect on mount if a token is already stored.
  $effect(() => {
    if ($adminToken && $adminConn.kind === 'disconnected' && !teardown) {
      attempted = true;
      teardown = startAdmin($adminToken);
    }
  });

  onDestroy(() => {
    if (teardown) teardown();
  });

  function handleConnect(): void {
    const t = tokenInput.trim();
    if (!t) return;
    attempted = true;
    adminToken.set(t);
    if (teardown) teardown();
    teardown = startAdmin(t);
  }

  function handleDisconnect(): void {
    if (teardown) {
      teardown();
      teardown = null;
    }
    adminToken.set(null);
    tokenInput = '';
    attempted = false;
    stopAdmin();
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === 'Enter') handleConnect();
  }

  const status = $derived.by(() => {
    if ($adminConn.kind === 'authed') {
      const s = $adminConn.state.state;
      return s === 'lobby' ? 'lobby' : s === 'running' ? 'running' : 'ended';
    }
    if ($adminConn.kind === 'connecting') return 'connecting';
    if ($adminConn.kind === 'rejected') return 'rejected';
    return 'idle';
  });

  const canStart = $derived(
    $adminConn.kind === 'authed' && $adminConn.state.state === 'lobby',
  );
  const canAbort = $derived(
    $adminConn.kind === 'authed' && $adminConn.state.state === 'running',
  );
  const canReset = $derived(
    $adminConn.kind === 'authed' && $adminConn.state.state === 'ended',
  );
</script>

<div class="admin-panel">
  {#if $adminConn.kind === 'authed'}
    <span class="admin-badge admin-badge-{status}">admin · {status}</span>
    {#if $adminRoom?.last_winner}
      <span class="admin-winner" title="Most recent winner">
        winner: {$adminRoom.last_winner}
      </span>
    {/if}
    <button
      class="topbar-btn"
      type="button"
      disabled={!canStart}
      onclick={adminSendStart}
      title="Transition the room from lobby to running">Start</button>
    <button
      class="topbar-btn"
      type="button"
      disabled={!canAbort}
      onclick={adminSendAbort}
      title="Force-end the running match (no winner)">Abort</button>
    <button
      class="topbar-btn"
      type="button"
      disabled={!canReset}
      onclick={adminSendReset}
      title="Cut the post-game pause short and return to lobby">Reset</button>
    <button
      class="topbar-btn admin-disconnect"
      type="button"
      onclick={handleDisconnect}
      title="Forget the admin token and close the admin connection">Logout</button>
  {:else}
    <input
      type="password"
      class="admin-token-input"
      placeholder="Admin token"
      autocomplete="off"
      bind:value={tokenInput}
      onkeydown={onKeyDown}
    />
    <button
      class="topbar-btn"
      type="button"
      onclick={handleConnect}
      disabled={!tokenInput.trim()}>Connect</button>
    {#if attempted && $adminConn.kind === 'rejected'}
      <span class="admin-rejected" title={$adminConn.reason}>auth failed</span>
    {:else if $adminConn.kind === 'connecting'}
      <span class="admin-connecting">connecting…</span>
    {/if}
  {/if}
</div>
