<script lang="ts">
  import BotList from './BotList.svelte';
  import EventLog from './EventLog.svelte';
</script>
<aside id="sidebar">
  <section class="roster-region"><h2>Teams · select to inspect</h2><BotList /></section>
  <section class="event-region"><h2>Recent events</h2><EventLog /></section>
</aside>
