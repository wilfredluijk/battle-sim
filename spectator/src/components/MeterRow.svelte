<!-- Horizontal labelled bar used for HP / ammo readouts. -->
<script lang="ts">
  interface Props {
    label: string;
    value: number;
    max: number;
    fill: string;
    valueText: string;
  }

  let { label, value, max, fill, valueText }: Props = $props();

  const pct = $derived(Math.max(0, Math.min(1, value / max)) * 100);
</script>

<div class="meter-row">
  <span class="meter-key">{label}</span>
  <div class="meter-track">
    <div class="meter-fill" style="width: {pct}%; background: {fill};"></div>
  </div>
  <span class="meter-value">{valueText}</span>
</div>
