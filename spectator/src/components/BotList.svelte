<script lang="ts">
  import { bots } from '../stores';
  import { botsInOrder } from '../lib/worldFrame';
  import BotCard from './BotCard.svelte';

  const ordered = $derived(botsInOrder($bots));
</script>

<ul class="bots">
  {#each ordered as card (card.ship.id)}
    <BotCard {card} />
  {/each}
</ul>
