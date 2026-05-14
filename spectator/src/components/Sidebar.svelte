<script lang="ts">
  import { tick } from '../stores';
  import BotList from './BotList.svelte';
  import EventLog from './EventLog.svelte';
</script>

<aside id="sidebar">
  <section>
    <h2>Tick</h2>
    <div id="tick" class="tick">{$tick || '—'}</div>
  </section>
  <section>
    <h2>Bots</h2>
    <BotList />
  </section>
  <section>
    <h2>Recent events</h2>
    <EventLog />
  </section>
</aside>
