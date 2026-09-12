<script lang="ts">
  import { adminToken, loginAdmin, logoutAdmin } from '../stores/admin';
  let password = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);
  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (!password || busy) return;
    busy = true; error = null;
    try { await loginAdmin(password); password = ''; }
    catch (e) { error = e instanceof Error ? e.message : 'Sign-in failed'; }
    finally { busy = false; }
  }
</script>
{#if $adminToken}
  <button class="topbar-btn quiet" type="button" onclick={logoutAdmin}>Sign out</button>
{:else}
  <form class="login-form" onsubmit={submit}>
    <label for="admin-password">Administrator password</label>
    <input id="admin-password" type="password" class="config-input" autocomplete="current-password" bind:value={password} disabled={busy} required />
    <button class="pm-start" type="submit" disabled={busy || !password}>{busy ? 'Signing in…' : 'Sign in'}</button>
    {#if error}<p class="config-err" role="alert">{error}</p>{/if}
  </form>
{/if}
