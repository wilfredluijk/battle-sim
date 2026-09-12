<!-- Replay viewer: a paused-by-default battlefield with a scrubbable timeline, event
     markers, a per-bot perspective selector, and an exit back to the live screen. -->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import ReplayCanvas from './ReplayCanvas.svelte';
  import {
    advanceTick,
    replayError,
    stepFrame,
    exitReplay,
    replayData,
    replayLoading,
    replayMarkers,
    replayPerspective,
    replayPlaying,
    replaySpeed,
    replayTick,
    seekTo,
    selectPerspective,
    togglePlay,
  } from '../stores/replay';

  import { eventText, clockText } from '../lib/presentation';
  let bookmarks = $state<number[]>([]);
  const names = $derived(new Map($replayData?.header.bots.map(b => [b.ship_id, b.name]) ?? []));
  const currentEvents = $derived($replayData?.frames[$replayTick]?.events ?? []);
  function eventJump(direction: number) {
    const ticks = [...new Set($replayMarkers.map(m => m.tick))];
    const target = direction < 0 ? ticks.filter(t => t < $replayTick).slice(-1)[0] : ticks.find(t => t > $replayTick);
    if (target != null) { replayPlaying.set(false); seekTo(target); }
  }
  function step(delta: number) { replayPlaying.set(false); stepFrame(delta); }
  onDestroy(() => replayPlaying.set(false));
  const SPEEDS = [0.25, 0.5, 1, 2, 4];

  // Playback ticker — alive only while playing, re-created when speed changes. The cleanup
  // return clears the interval on pause, speed change, or unmount.
  $effect(() => {
    if (!$replayPlaying || !$replayData) return;
    const hz = Math.max(1, $replayData.header.tick_hz) * $replaySpeed;
    const id = setInterval(() => advanceTick(), 1000 / hz);
    return () => clearInterval(id);
  });

  const max = $derived($replayData ? $replayData.frames.length - 1 : 0);
  const seconds = $derived(
    $replayData
      ? ($replayTick / Math.max(1, $replayData.header.tick_hz)).toFixed(1)
      : '0.0',
  );

  function onScrub(e: Event): void {
    seekTo(Number((e.currentTarget as HTMLInputElement).value));
  }
  function onPerspective(e: Event): void {
    void selectPerspective((e.currentTarget as HTMLSelectElement).value);
  }
</script>

<main class="replay-viewer">
  <div class="rv-bar rv-top">
    <button class="topbar-btn" type="button" onclick={exitReplay}>Exit replay</button>
    <span class="rv-title">
      {#if $replayData}{$replayData.header.bots.map((b) => b.name).join('  vs  ')}{/if}
    </span>
    <span class="topbar-spacer"></span>
    {#if $replayLoading}<span class="rv-loading">loading…</span>{/if}
    <label class="rv-field">
      Perspective
      <select value={$replayPerspective} onchange={onPerspective}>
        <option value="overall">Overall (ground truth)</option>
        {#if $replayData}
          {#each $replayData.header.bots as b (b.bot_id)}
            <option value={b.bot_id}>{b.name}</option>
          {/each}
        {/if}
      </select>
    </label>
  </div>

  {#if $replayError}<p class="config-err" role="alert">{$replayError}. Showing overall ground truth. Choose a perspective to retry.</p>{/if}
  <div class="rv-stage">
    <ReplayCanvas />
  </div>

  <div class="rv-bar rv-controls">
    <button
      class="rv-play"
      type="button"
      onclick={togglePlay}
      aria-label={$replayPlaying ? 'pause' : 'play'}
    >
      {$replayPlaying ? '❚❚' : '▶'}
    </button>

    <div class="rv-track">
      <input
        class="rv-slider"
        type="range"
        min="0"
        {max}
        step="1"
        value={$replayTick}
        oninput={onScrub}
        aria-label="replay tick"
      />
      <div class="rv-markers" aria-hidden="true">
        {#each $replayMarkers as m, i (i)}
          <span
            class="rv-marker rv-marker-{m.kind}"
            style="left: {max > 0 ? (m.tick / max) * 100 : 0}%"
            title="tick {m.tick}: {m.kind}"
          ></span>
        {/each}
      </div>
    </div>

    <span class="rv-readout">{$replayTick} / {max} · {seconds}s</span>

    <label class="rv-field">
      Speed
      <select bind:value={$replaySpeed}>
        {#each SPEEDS as s (s)}
          <option value={s}>{s}×</option>
        {/each}
      </select>
    </label>
  </div>

  <div class="rv-debrief">
    <button class="topbar-btn" onclick={() => eventJump(-1)}>Previous event</button><button class="topbar-btn" onclick={() => step(-1)} aria-label="Previous frame">−1 frame</button><button class="topbar-btn" onclick={() => step(1)} aria-label="Next frame">+1 frame</button><button class="topbar-btn" onclick={() => eventJump(1)}>Next event</button>
    <button class="topbar-btn" onclick={() => { if (!bookmarks.includes($replayTick)) bookmarks = [...bookmarks, $replayTick].sort((a,b) => a-b); }}>Bookmark</button>
    {#each bookmarks as mark (mark)}<button class="topbar-btn" onclick={() => { replayPlaying.set(false); seekTo(mark); }}>{clockText(mark / ($replayData?.header.tick_hz ?? 1))}</button>{/each}
    {#each currentEvents as event, i (i)}<span>{eventText(event, names)}</span>{/each}
  </div>
  <div class="rv-legend" aria-hidden="true">
    <span><i class="rv-dot rv-marker-fired"></i> shot fired</span>
    <span><i class="rv-dot rv-marker-hit"></i> hit</span>
    <span><i class="rv-dot rv-marker-kill"></i> kill</span>
    <span><i class="rv-dot rv-marker-powerup"></i> powerup</span>
  </div>
</main>
