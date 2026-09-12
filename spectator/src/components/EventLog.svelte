<script lang="ts">
  import { structuredEvents, bots, selectedTeam } from '../stores';
  import { room } from '../stores/admin';
  import { clockText, eventText } from '../lib/presentation';
  let filter = $state('important');
  const names = $derived(new Map([...$bots.values()].map(b => [b.ship.id, b.ship.bot_name])));
  const filtered = $derived($structuredEvents.filter(e => filter === 'all' || (filter === 'important' && e.event.type !== 'shell_splash') || e.event.type === filter));
</script>
<label class="event-filter">Show <select bind:value={filter}><option value="important">Combat & connections</option><option value="all">All events</option><option value="disconnect">Disconnects</option><option value="hit">Hits</option><option value="death">Eliminations</option><option value="powerup_activated">Powerups</option></select></label>
<ul class="events">
  {#each filtered as item, i (i)}
    <li><button class="event-link" onclick={() => { if ('ship_id' in item.event) selectedTeam.set(item.event.ship_id); }}>
      <time>{$room?.tick_hz ? clockText(item.tick / $room.tick_hz) : `Tick ${item.tick}`}</time> {eventText(item.event, names)}
    </button></li>
  {:else}<li>No events yet.</li>{/each}
</ul>
