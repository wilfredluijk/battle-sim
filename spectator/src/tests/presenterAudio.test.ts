import { describe, expect, it, vi } from 'vitest';
import { LiveCueTracker, PresenterAudio, soundPreferences, type AudioStatus } from '../lib/presenterAudio';
import type { ShipSnapshot, WorldFrame } from '../types/protocol';

function frame(tick: number, deaths: string[] = []): WorldFrame {
  return { type: 'world', tick, ships: [], shells: [], events: deaths.map(ship_id => ({ type: 'death', ship_id })) };
}
describe('live sound history', () => {
  it('silently establishes a baseline and coalesces simultaneous deaths without repeats', () => {
    const tracker = new LiveCueTracker();
    expect(tracker.observe(frame(30, ['old']), 'round-1', true, 0)).toBeNull();
    expect(tracker.observe(frame(31, ['a', 'b']), 'round-1', true, 100)).toBe('elimination');
    expect(tracker.observe(frame(31, ['a', 'b']), 'round-1', true, 110)).toBeNull();
    expect(tracker.observe(frame(32, ['old', 'a']), 'round-1', true, 200)).toBeNull();
  });
  it('does not catch up missed frames or events after a stale connection', () => {
    const tracker = new LiveCueTracker();
    tracker.observe(frame(1), 'r', true, 0);
    expect(tracker.observe(frame(4, ['a']), 'r', true, 100)).toBeNull();
    expect(tracker.observe(frame(5, ['b']), 'r', true, 4000)).toBeNull();
    expect(tracker.observe(frame(6, ['c']), 'r', true, 4100)).toBe('elimination');
    tracker.reset();
    expect(tracker.observe(frame(7, ['d']), 'r', true, 4200)).toBeNull();
  });
  it('resets on match changes, rewinds and leaving the eligible live view', () => {
    const tracker = new LiveCueTracker();
    tracker.observe(frame(1), 'r', true, 0);
    expect(tracker.observe(frame(2, ['a']), 'r', false, 100)).toBeNull();
    expect(tracker.observe(frame(3, ['b']), 'r', true, 200)).toBeNull();
    expect(tracker.observe(frame(4, ['c']), 'next', true, 300)).toBeNull();
    expect(tracker.observe(frame(0, ['d']), 'next', true, 400)).toBeNull();
    expect(tracker.observe(frame(1, ['c']), 'next', true, 500)).toBe('elimination');
    expect(tracker.observe(frame(2, ['e']), undefined, true, 600)).toBeNull();
  });
  it('absorbs destroyed hulls without calling them new eliminations or forfeits', () => {
    const tracker = new LiveCueTracker();
    const baseline = frame(1);
    baseline.ships = [{ id: 'a', alive: false }] as ShipSnapshot[];
    tracker.observe(baseline, 'r', true, 0);
    expect(tracker.observe(frame(2, ['a']), 'r', true, 100)).toBeNull();
    expect(tracker.observe(frame(3), 'r', true, 200)).toBeNull();
  });
});

function fakeAudio() {
  const gains: ReturnType<typeof gain>[] = [];
  const oscillators: ReturnType<typeof oscillator>[] = [];
  function gain() {
    return { gain: { value: 0, setValueAtTime: vi.fn(), linearRampToValueAtTime: vi.fn() }, connect: vi.fn(), disconnect: vi.fn() };
  }
  function oscillator() {
    return { type: '', frequency: { value: 0 }, connect: vi.fn(), disconnect: vi.fn(), start: vi.fn(), stop: vi.fn(), onended: null as (() => void) | null };
  }
  const context = {
    state: 'running', currentTime: 12, destination: {}, onstatechange: null as (() => void) | null,
    resume: vi.fn().mockResolvedValue(undefined), close: vi.fn().mockResolvedValue(undefined),
    createGain: () => { const node = gain(); gains.push(node); return node; },
    createOscillator: () => { const node = oscillator(); oscillators.push(node); return node; },
  };
  const factory = vi.fn(() => context as unknown as AudioContext);
  const statuses: AudioStatus[] = [];
  const audio = new PresenterAudio(status => statuses.push(status), factory);
  return { audio, context, factory, statuses, gains, oscillators };
}
describe('audio lifecycle', () => {
  it('starts muted without creating audio and bounds voices when cues overlap', async () => {
    const { audio, factory, oscillators, gains } = fakeAudio();
    expect(audio.play('countdown')).toBe(false); expect(factory).not.toHaveBeenCalled();
    await audio.enable();
    expect(audio.play('elimination')).toBe(true); expect(oscillators).toHaveLength(2);
    expect(gains[0].gain.value).toBe(0.25);
    expect(audio.play('countdown')).toBe(true);
    expect(oscillators[0].disconnect).toHaveBeenCalledOnce();
    expect(oscillators[1].disconnect).toHaveBeenCalledOnce();
    expect(oscillators[2].frequency.value).toBe(660);
    audio.mute();
    expect(oscillators[2].disconnect).toHaveBeenCalledOnce();
    expect(audio.play('countdown')).toBe(false);
  });
  it('cannot become enabled when a pending browser resume resolves after mute', async () => {
    const { audio, context, statuses, oscillators } = fakeAudio();
    let resolve!: () => void;
    context.resume.mockImplementation(() => new Promise<void>(yes => resolve = yes));
    const pending = audio.enable();
    audio.mute(); resolve(); await pending;
    expect(statuses[statuses.length - 1]).toEqual({ enabled: false, pending: false, error: null });
    expect(audio.play('elimination')).toBe(false); expect(oscillators).toHaveLength(0);
  });
  it('shows a recoverable failure and mutes when the browser suspends audio', async () => {
    const { audio, context, statuses } = fakeAudio();
    context.resume.mockRejectedValueOnce(new Error('Blocked'));
    await audio.enable(); expect(statuses[statuses.length - 1]?.error).toContain('could not start');
    await audio.enable(); expect(statuses[statuses.length - 1]?.enabled).toBe(true);
    context.state = 'suspended'; context.onstatechange?.();
    expect(statuses[statuses.length - 1]?.enabled).toBe(false); expect(statuses[statuses.length - 1]?.error).toContain('paused');
  });
  it('handles browsers without Web Audio and playback failures without throwing', async () => {
    const changed = vi.fn();
    await new PresenterAudio(changed, () => { throw new Error('Unavailable'); }).enable();
    expect(changed.mock.lastCall?.[0]).toMatchObject({ enabled: false, pending: false, error: expect.any(String) });
    const { audio, context, statuses } = fakeAudio();
    await audio.enable(); context.createOscillator = () => { throw new Error('Device lost'); };
    expect(audio.play('elimination')).toBe(false); expect(statuses[statuses.length - 1]?.error).toContain('playback failed');
  });
  it('applies volume immediately, releases finished notes and closes on teardown', async () => {
    const { audio, context, gains, oscillators } = fakeAudio();
    await audio.enable(); audio.play('countdown');
    audio.setVolume(0); expect(gains[0].gain.setValueAtTime).toHaveBeenLastCalledWith(0, 12);
    expect(audio.play('elimination')).toBe(false);
    oscillators[0].onended?.(); expect(oscillators[0].disconnect).toHaveBeenCalledOnce();
    audio.dispose(); expect(context.close).toHaveBeenCalledOnce(); expect(context.onstatechange).toBeNull();
  });
  it('bounds saved preferences and never accepts a persisted enabled state', () => {
    expect(soundPreferences({ volume: 9, countdown: false, eliminations: 'yes', enabled: true })).toEqual({ volume: 1, countdown: false, eliminations: true });
    expect(soundPreferences({ volume: -1 }).volume).toBe(0);
    expect(soundPreferences({ volume: NaN }).volume).toBe(0.25);
    expect(soundPreferences(null)).toEqual(soundPreferences('invalid'));
  });
});
