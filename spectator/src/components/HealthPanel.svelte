<script lang="ts">
  import { onMount } from 'svelte';
  import { storedAuthHeaders } from '../lib/authToken';
  import { adminToken, expireSession } from '../stores/admin';
  import { get } from 'svelte/store';
  let sample = $state<{ accepted: number; rejected: number; step: number | null } | null>(null);
  let error = $state('');
  onMount(() => {
    let previous: { commands_accepted: number; commands_rejected: number; ticks: number; step_total_us: number; at: number } | null = null;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const token = get(adminToken);
        if (!token) return;
        const res = await fetch('/api/metrics', { headers: storedAuthHeaders() });
        if (stopped || token !== get(adminToken)) return;
        if (res.status === 401) { expireSession(); return; }
        if (!res.ok) throw new Error('Metrics unavailable');
        const next = { ...await res.json(), at: performance.now() };
        if (stopped) return;
        if (previous && next.ticks >= previous.ticks && next.commands_accepted >= previous.commands_accepted) {
          const seconds = (next.at - previous.at) / 1000;
          const ticks = next.ticks - previous.ticks;
          sample = { accepted: (next.commands_accepted - previous.commands_accepted) / seconds, rejected: Math.max(0, next.commands_rejected - previous.commands_rejected) / seconds, step: ticks ? (next.step_total_us - previous.step_total_us) / ticks / 1000 : null };
        } else sample = null;
        previous = next; error = '';
      } catch { if (!stopped) { error = 'Metrics unavailable; retrying.'; sample = null; } }
      if (!stopped) timer = setTimeout(poll, 3000);
    }
    void poll(); return () => { stopped = true; clearTimeout(timer); };
  });
</script>
<section class="pm-panel"><h2>Trainer health · recent interval</h2>
  {#if error}<p class="config-err">{error}</p>{:else if sample}<p class="pm-sub">{sample.accepted.toFixed(1)} accepted commands/s · {sample.rejected.toFixed(1)} rejected/s</p><p class="pm-sub">Average simulation step: {sample.step === null ? 'no recent ticks' : `${sample.step.toFixed(2)} ms`}</p>{:else}<p class="pm-sub">Collecting a measurement interval…</p>{/if}
  <p class="config-note">Rejects include all rejection reasons. These counters do not measure team latency or late-command rate.</p>
</section>
