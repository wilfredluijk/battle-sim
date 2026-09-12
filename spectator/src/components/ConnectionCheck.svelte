<script lang="ts">
  import { room, roomError, roomUpdatedAt } from '../stores/admin';
  import TeamTiming from './TeamTiming.svelte';
  let remaining = $state<number | null>(null);
  let checkedAt = $state('');
  let results = $state<{ name: string; status: string }[]>([]);
  $effect(() => {
    if (remaining === null) return;
    if ($room?.state !== 'lobby') { remaining = null; return; }
    if (remaining === 0) {
      const bots = $room?.bots ?? [];
      const names = [...new Set([...($room?.expected_teams ?? []), ...bots.map(b => b.name)])];
      results = names.map(name => {
        const bot = bots.find(b => b.name === name);
        const fresh = bot?.diagnostics?.rtt_age_seconds != null && bot.diagnostics.rtt_age_seconds < 8;
        return { name, status: $roomError || Date.now() - $roomUpdatedAt > 5000 ? 'Server data unavailable' : !bot?.connected ? 'Not connected' : !bot.ready ? 'Rules acknowledgement needed' : !fresh ? 'No recent heartbeat response' : 'Ready · heartbeat received' };
      });
      checkedAt = new Date().toLocaleTimeString(); remaining = null; return;
    }
    const timer = setTimeout(() => remaining = remaining === null ? null : remaining - 1, 1000);
    return () => clearTimeout(timer);
  });
</script>
<section class="pm-panel"><h2>Venue preflight</h2>
  <p class="pm-sub">Observe team readiness and heartbeats for 15 seconds on the current connections.</p>
  {#if remaining !== null}<p role="status">Checking… {remaining}s remaining</p><button class="topbar-btn" onclick={() => remaining = null}>Cancel check</button>
  {:else}<button class="topbar-btn" disabled={$room?.state !== 'lobby' || !!$roomError} onclick={() => { results = []; checkedAt = ''; remaining = 15; }}>Run connection check</button>{/if}
  {#if checkedAt}<p role="status">Checked at {checkedAt}</p><ul class="check-results">{#each results as r (r.name)}<li><strong>{r.name}</strong> · {r.status}</li>{/each}</ul>{/if}
  <p class="config-note">This checks transport and readiness. Use a practice round to measure command response and deadline misses under match load.</p>
  <TeamTiming bots={$room?.bots ?? []} />
</section>
