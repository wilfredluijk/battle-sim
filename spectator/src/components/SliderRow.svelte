<!-- Horizontal bipolar slider used for throttle / rudder / speed readouts. The fill grows
     from the centre toward whichever side the value points. -->
<script lang="ts">
  interface Props {
    label: string;
    /** Value in `[-1, 1]` (or any clipped range you pass via `min`/`max`). */
    value: number;
    valueText: string;
    /** Colour for positive fills. Negative fills are always amber. */
    positiveColor?: string;
  }

  let { label, value, valueText, positiveColor = '#6cb1ff' }: Props = $props();

  const clamped = $derived(Math.max(-1, Math.min(1, value || 0)));
  const widthPct = $derived(Math.abs(clamped) * 50);
  const leftPct = $derived(clamped >= 0 ? 50 : 50 - widthPct);
  const color = $derived(clamped >= 0 ? positiveColor : '#ef9a4a');
</script>

<div class="meter-row">
  <span class="meter-key">{label}</span>
  <div class="slider-track">
    <div class="slider-center"></div>
    <div class="slider-fill" style="left: {leftPct}%; width: {widthPct}%; background: {color};"></div>
  </div>
  <span class="meter-value">{valueText}</span>
</div>
