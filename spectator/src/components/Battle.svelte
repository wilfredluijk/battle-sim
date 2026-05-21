<!-- Battle overview: the live battlefield canvas plus the bot/event sidebar. Shown only
     while a match is running. -->
<script lang="ts">
  import Battlefield from './Battlefield.svelte';
  import Sidebar from './Sidebar.svelte';
  import { view } from '../stores';

  const layoutClass = $derived($view === 'full' ? 'layout-full' : 'layout-split');
</script>

<main class={layoutClass}>
  <Battlefield />
  <Sidebar />
</main>
