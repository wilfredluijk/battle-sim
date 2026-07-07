<!-- Canvas wrapper. Owns the requestAnimationFrame loop, DPR fitting, and
     view-mode-driven canvas resize. All drawing delegates to `lib/renderer.ts`. -->
<script lang="ts">
  import { onDestroy, onMount, tick as sveltetick } from 'svelte';
  import { latestWorld, splashes, view } from '../stores';
  import { draw, type Splash } from '../lib/renderer';
  import { fitCanvas } from '../lib/canvas';
  import { room } from '../stores/admin';
  import { MAP_WIDTH, MAP_HEIGHT, MAX_HP, ACTIVE_RADAR_RANGE } from '../lib/constants';
  import type { WorldFrame } from '../types/protocol';

  let canvas: HTMLCanvasElement | null = $state(null);
  let rafId: number | null = null;
  let resizeObserver: ResizeObserver | null = null;

  // Local refs maintained by store subscriptions — the raf loop reads these directly
  // instead of re-subscribing each frame.
  let currentFrame: WorldFrame | null = null;
  let currentSplashes: Splash[] = [];

  const unsubFrame = latestWorld.subscribe((v) => (currentFrame = v));
  const unsubSplashes = splashes.subscribe((v) => (currentSplashes = v));

  // HP-bar scale follows the match's configured hull when known, so the canvas
  // matches the BotCard meters instead of assuming a fixed 100-HP hull. Map size follows
  // the room's actual `--map WxH` (default 700×700) so bounds and the letterbox transform
  // are correct on any map — the constants are only a pre-first-fetch fallback.
  let currentMaxHp = MAX_HP;
  let currentMapW = MAP_WIDTH;
  let currentMapH = MAP_HEIGHT;
  // Radar-ring radius follows the operator-tunable `active_radar_range` so the drawn ring
  // matches what bots can actually see; the constant is only a pre-first-fetch fallback.
  let currentRadarRange = ACTIVE_RADAR_RANGE;
  const unsubRoom = room.subscribe((r) => {
    currentMaxHp = r?.config?.hull_hp ?? MAX_HP;
    currentMapW = r?.map?.width ?? MAP_WIDTH;
    currentMapH = r?.map?.height ?? MAP_HEIGHT;
    currentRadarRange = r?.config?.active_radar_range ?? ACTIVE_RADAR_RANGE;
  });

  // View toggle changes the canvas's CSS size; the pixel buffer must be re-synced AFTER
  // the layout settles. `await sveltetick()` defers to the next microtask once Svelte
  // has applied DOM mutations.
  const unsubView = view.subscribe(async () => {
    await sveltetick();
    if (canvas) fitCanvas(canvas);
  });

  onMount(() => {
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    fitCanvas(canvas);

    // ResizeObserver fires on layout changes too, not just window-size changes — covers
    // DPI changes, manual window resize, and the split↔full toggle uniformly.
    resizeObserver = new ResizeObserver(() => {
      if (canvas) fitCanvas(canvas);
    });
    resizeObserver.observe(canvas);

    const loop = (): void => {
      rafId = requestAnimationFrame(loop);
      draw(
        ctx,
        currentFrame,
        currentSplashes,
        performance.now(),
        currentMapW,
        currentMapH,
        currentMaxHp,
        currentRadarRange,
      );
    };
    rafId = requestAnimationFrame(loop);
  });

  onDestroy(() => {
    if (rafId != null) cancelAnimationFrame(rafId);
    resizeObserver?.disconnect();
    unsubFrame();
    unsubSplashes();
    unsubView();
    unsubRoom();
  });
</script>

<canvas id="board" bind:this={canvas} aria-label="naval battle map" width="800" height="800"
></canvas>
