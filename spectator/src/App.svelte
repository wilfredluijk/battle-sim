<script lang="ts">
  import { onDestroy } from 'svelte';
  import Topbar from './components/Topbar.svelte';
  import Battlefield from './components/Battlefield.svelte';
  import Sidebar from './components/Sidebar.svelte';
  import { startSpectator, view } from './stores';

  const teardown = startSpectator();
  onDestroy(teardown);

  const layoutClass = $derived($view === 'full' ? 'layout-full' : 'layout-split');
</script>

<Topbar />
<main class={layoutClass}>
  <Battlefield />
  <Sidebar />
</main>
