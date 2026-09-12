<script lang="ts">
  import type { TeamDiagnostics } from '../types/protocol';
  let { bots, historical = false }: { bots: { name: string; diagnostics?: TeamDiagnostics }[]; historical?: boolean } = $props();
  const ms = (n: number | null | undefined) => n == null ? '—' : n.toFixed(1);
</script>
<details class="team-timing">
  <summary>Per-team timing {historical ? '· recorded at match end' : '· latest measurements'}</summary>
  <p class="config-note">RTT measures WebSocket ping/pong on each team’s connection. Response measures server tick send to first matching command, including network travel and bot processing. Percentiles use the latest 512 samples; command counters reset each match. Missing samples show —.</p>
  <div class="table-scroll"><table class="session-table"><thead><tr><th>Team</th><th>RTT p50 / p95 ms</th><th>Response p50 / p95 ms</th><th>Responses</th><th>Late rejects</th><th>Late-window rate</th><th>Wrong tick</th><th>Missed windows</th></tr></thead><tbody>{#each bots as bot (bot.name)}<tr><th>{bot.name}</th><td>{ms(bot.diagnostics?.rtt.p50_ms)} / {ms(bot.diagnostics?.rtt.p95_ms)}</td><td>{ms(bot.diagnostics?.response.p50_ms)} / {ms(bot.diagnostics?.response.p95_ms)}</td><td>{bot.diagnostics?.response.samples ?? 0}</td><td>{bot.diagnostics?.late ?? '—'}</td><td>{bot.diagnostics?.completed_windows ? `${(100 * bot.diagnostics.late_windows / bot.diagnostics.completed_windows).toFixed(1)}%` : "—"}</td><td>{bot.diagnostics?.wrong_tick ?? '—'}</td><td>{bot.diagnostics ? `${bot.diagnostics.missed_windows} / ${bot.diagnostics.completed_windows}` : '—'}</td></tr>{/each}</tbody></table></div>
  <p class="config-note">A missed window received no accepted command before closing. Late rejects count only matching-tick commands after the deadline; wrong-tick rejects are separate. Late-window rate is the share of closed command windows with a late matching command and no accepted command. Repeated rejects can exceed missed windows. RTT samples span the connection and may predate this match.</p>
</details>
