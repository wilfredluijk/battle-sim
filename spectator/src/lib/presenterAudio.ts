import type { WorldFrame } from '../types/protocol';

export type PresenterCue = 'countdown' | 'elimination';
export interface SoundPreferences { volume: number; countdown: boolean; eliminations: boolean }
export interface AudioStatus { enabled: boolean; pending: boolean; error: string | null }
export const defaultSoundPreferences: SoundPreferences = { volume: 0.25, countdown: true, eliminations: true };

export function soundPreferences(value: unknown): SoundPreferences {
  const input = value && typeof value === 'object' ? value as Partial<SoundPreferences> : {};
  return {
    volume: typeof input.volume === 'number' && Number.isFinite(input.volume)
      ? Math.min(1, Math.max(0, input.volume)) : defaultSoundPreferences.volume,
    countdown: typeof input.countdown === 'boolean' ? input.countdown : true,
    eliminations: typeof input.eliminations === 'boolean' ? input.eliminations : true,
  };
}

/** Observe only fresh, consecutive live frames. A first frame is always a silent
 * baseline, including after reconnect, navigation, unmute or a match change. */
export class LiveCueTracker {
  private previous: { matchId: string; tick: number; at: number } | null = null;
  private deaths = new Set<string>();

  reset(): void { this.previous = null; this.deaths.clear(); }

  observe(frame: WorldFrame, matchId: string | undefined, eligible: boolean, now: number): PresenterCue | null {
    if (!eligible || !matchId) { this.reset(); return null; }
    const previous = this.previous;
    if (previous?.matchId !== matchId || frame.tick < previous.tick) this.reset();
    if (previous?.matchId === matchId && frame.tick === previous.tick) return null;
    const consecutive = this.previous !== null && frame.tick === this.previous.tick + 1 && now - this.previous.at < 3000;
    this.previous = { matchId, tick: frame.tick, at: now };
    let eliminated = false;
    for (const event of frame.events) {
      if (event.type !== 'death') continue;
      if (!this.deaths.has(event.ship_id)) eliminated = true;
      this.deaths.add(event.ship_id);
    }
    // Also absorb previously destroyed hulls, so an old death never becomes a new cue.
    for (const ship of frame.ships) if (!ship.alive) this.deaths.add(ship.id);
    return consecutive && eliminated ? 'elimination' : null;
  }
}

/** Short synthesized tones with no downloads, queues or autoplay. The caller must
 * invoke enable() from a user gesture. Muting invalidates a pending resume too. */
export class PresenterAudio {
  private context: AudioContext | null = null;
  private master: GainNode | null = null;
  private voices = new Set<{ oscillator: OscillatorNode; envelope: GainNode }>();
  private generation = 0;
  private volume = defaultSoundPreferences.volume;
  private current: AudioStatus = { enabled: false, pending: false, error: null };

  constructor(
    private readonly changed: (status: AudioStatus) => void,
    private readonly createContext: () => AudioContext = () => new AudioContext(),
  ) {}

  private publish(status: AudioStatus): void { this.current = status; this.changed(status); }

  async enable(): Promise<void> {
    if (this.current.pending || this.current.enabled) return;
    const generation = ++this.generation;
    this.publish({ enabled: false, pending: true, error: null });
    try {
      if (!this.context || this.context.state === 'closed') {
        const context = this.createContext();
        this.context = context;
        this.master = context.createGain();
        this.master.gain.value = this.volume;
        this.master.connect(context.destination);
        context.onstatechange = () => {
          if (this.context === context && this.current.enabled && context.state !== 'running') {
            this.mute('Audio paused by the browser. Enable sounds again to continue.');
          }
        };
      }
      await this.context.resume();
      if (generation !== this.generation) return;
      if (this.context.state !== 'running') throw new Error('Audio is not running');
      this.publish({ enabled: true, pending: false, error: null });
    } catch {
      if (generation === this.generation) this.mute('Sound could not start in this browser. Try enabling it again.');
    }
  }

  setVolume(volume: number): void {
    this.volume = soundPreferences({ volume }).volume;
    if (this.master && this.context) this.master.gain.setValueAtTime(this.volume, this.context.currentTime);
  }

  stop(): void {
    for (const { oscillator, envelope } of this.voices) {
      oscillator.onended = null;
      try { oscillator.stop(); } catch { /* A completed oscillator may already be stopped. */ }
      oscillator.disconnect(); envelope.disconnect();
    }
    this.voices.clear();
  }

  mute(error: string | null = null): void {
    ++this.generation;
    this.stop();
    this.publish({ enabled: false, pending: false, error });
  }

  play(cue: PresenterCue): boolean {
    const context = this.context;
    if (!this.current.enabled || !context || !this.master || context.state !== 'running' || this.volume === 0) return false;
    this.stop(); // Simultaneous eliminations make one cue; never build an audio backlog.
    try {
      const notes = cue === 'countdown' ? [[660, 0, 0.10]] : [[440, 0, 0.14], [294, 0.16, 0.20]];
      for (const [frequency, offset, duration] of notes) {
        const oscillator = context.createOscillator();
        const envelope = context.createGain();
        const voice = { oscillator, envelope };
        this.voices.add(voice);
        const at = context.currentTime + offset;
        oscillator.type = 'sine'; oscillator.frequency.value = frequency;
        envelope.gain.setValueAtTime(0, at);
        envelope.gain.linearRampToValueAtTime(0.18, at + 0.01);
        envelope.gain.linearRampToValueAtTime(0, at + duration);
        oscillator.connect(envelope); envelope.connect(this.master);
        oscillator.onended = () => {
          oscillator.disconnect(); envelope.disconnect(); this.voices.delete(voice);
        };
        oscillator.start(at); oscillator.stop(at + duration + 0.01);
      }
      return true;
    } catch {
      this.mute('Sound playback failed. Enable sounds to try again.');
      return false;
    }
  }

  dispose(): void {
    this.mute();
    const context = this.context;
    this.context = null; this.master = null;
    if (context) { context.onstatechange = null; void context.close().catch(() => {}); }
  }
}
