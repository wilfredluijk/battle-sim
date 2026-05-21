<!-- Canvas for the replay viewer. Owns its own requestAnimationFrame loop and draws the
     frame at the current replay tick — either the ground-truth world or, when a bot
     perspective is selected, that bot's sensor-filtered view. Independent of the live
     spectator canvas in Battlefield.svelte. -->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { fitCanvas } from '../lib/canvas';
  import { draw, drawPerspective } from '../lib/renderer';
  import {
    replayData,
    replayPerspective,
    replayPerspectiveData,
    replayTick,
    type Perspective,
  } from '../stores/replay';
  import type { CapturedPerspective, CapturedReplay } from '../types/protocol';

  let canvas: HTMLCanvasElement | null = $state(null);
  let rafId: number | null = null;
  let resizeObserver: ResizeObserver | null = null;

  // Local refs the raf loop reads directly, kept current by store subscriptions.
  let data: CapturedReplay | null = null;
  let tick = 0;
  let perspective: Perspective = 'overall';
  let perspectiveData: CapturedPerspective | null = null;

  const unsubData = replayData.subscribe((v) => (data = v));
  const unsubTick = replayTick.subscribe((v) => (tick = v));
  const unsubPersp = replayPerspective.subscribe((v) => (perspective = v));
  const unsubPerspData = replayPerspectiveData.subscribe((v) => (perspectiveData = v));

  function render(ctx: CanvasRenderingContext2D): void {
    if (!data) {
      draw(ctx, null, [], performance.now());
      return;
    }
    const mapW = data.header.map.width;
    const mapH = data.header.map.height;
    const frame = data.frames[Math.max(0, Math.min(tick, data.frames.length - 1))] ?? null;

    if (perspective === 'overall' || !perspectiveData) {
      // Replay frames are drawn directly (no splash interpolation, which assumes
      // monotonic time and would misbehave on a slider seek).
      draw(ctx, frame, [], performance.now(), mapW, mapH);
      return;
    }

    const bot = data.header.bots.find((b) => b.bot_id === perspective);
    const ownShip =
      bot && frame ? (frame.ships.find((s) => s.id === bot.ship_id) ?? null) : null;
    const pf =
      perspectiveData.frames[
        Math.max(0, Math.min(tick, perspectiveData.frames.length - 1))
      ];
    drawPerspective(ctx, ownShip, pf?.contacts ?? [], mapW, mapH);
  }

  onMount(() => {
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    fitCanvas(canvas);
    resizeObserver = new ResizeObserver(() => {
      if (canvas) fitCanvas(canvas);
    });
    resizeObserver.observe(canvas);

    const loop = (): void => {
      rafId = requestAnimationFrame(loop);
      render(ctx);
    };
    rafId = requestAnimationFrame(loop);
  });

  onDestroy(() => {
    if (rafId != null) cancelAnimationFrame(rafId);
    resizeObserver?.disconnect();
    unsubData();
    unsubTick();
    unsubPersp();
    unsubPerspData();
  });
</script>

<canvas
  class="replay-board"
  bind:this={canvas}
  width="800"
  height="800"
  aria-label="replay battlefield"
></canvas>
