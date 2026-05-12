<script lang="ts">
  import { events } from '../stores';
</script>

<ul class="events">
  {#each $events as line, i (i + ':' + line)}
    <li>{line}</li>
  {/each}
</ul>
