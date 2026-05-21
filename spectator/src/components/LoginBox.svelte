<!-- Admin login control in the Topbar. Logged out: a password field that POSTs to
     /api/login. Logged in: an "admin" badge and a Logout button. -->
<script lang="ts">
  import { adminToken, loginAdmin, logoutAdmin } from '../stores/admin';

  let password = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit(): Promise<void> {
    const p = password.trim();
    if (!p || busy) return;
    busy = true;
    error = null;
    try {
      await loginAdmin(p);
      password = '';
    } catch (e) {
      error = e instanceof Error ? e.message : 'login failed';
    } finally {
      busy = false;
    }
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (e.key === 'Enter') void submit();
  }
</script>

<div class="admin-panel">
  {#if $adminToken}
    <span class="admin-badge admin-badge-lobby">admin</span>
    <button
      class="topbar-btn admin-disconnect"
      type="button"
      onclick={logoutAdmin}
      title="Forget the admin token">Logout</button>
  {:else}
    <input
      type="password"
      class="admin-token-input"
      placeholder="Admin password"
      autocomplete="off"
      bind:value={password}
      onkeydown={onKeyDown}
      disabled={busy}
    />
    <button
      class="topbar-btn"
      type="button"
      onclick={submit}
      disabled={busy || !password.trim()}>Login</button>
    {#if error}
      <span class="admin-rejected" title={error}>{error}</span>
    {/if}
  {/if}
</div>
