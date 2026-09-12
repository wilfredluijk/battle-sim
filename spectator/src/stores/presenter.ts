import { get, writable } from 'svelte/store';
import { LiveCueTracker, PresenterAudio, soundPreferences, type AudioStatus, type PresenterCue, type SoundPreferences } from '../lib/presenterAudio';
import type { WorldFrame } from '../types/protocol';

const storageKey = 'naval-presenter-sounds';
function savedPreferences(): SoundPreferences {
  try { return soundPreferences(JSON.parse(localStorage.getItem(storageKey) ?? 'null')); }
  catch { return soundPreferences(null); }
}
export const soundSettings = writable(savedPreferences());
export const soundStatus = writable<AudioStatus>({ enabled: false, pending: false, error: null });
const tracker = new LiveCueTracker();
const audio = new PresenterAudio(status => { tracker.reset(); soundStatus.set(status); });
audio.setVolume(get(soundSettings).volume);

export function updateSoundSettings(patch: Partial<SoundPreferences>): void {
  const settings = soundPreferences({ ...get(soundSettings), ...patch });
  soundSettings.set(settings); audio.setVolume(settings.volume);
  resetLiveCues();
  try { localStorage.setItem(storageKey, JSON.stringify(settings)); } catch { /* Preferences still work for this page. */ }
}
export function enableSounds(): Promise<void> { return audio.enable(); }
export function muteSounds(): void { audio.mute(); }
export function resetLiveCues(): void { tracker.reset(); audio.stop(); }
export function countdownSound(): void {
  if (!document.hidden && get(soundSettings).countdown) audio.play('countdown');
}
export function testSound(cue: PresenterCue): boolean {
  return !document.hidden && audio.play(cue);
}
export function observeLiveSound(frame: WorldFrame, matchId: string | undefined, eligible: boolean, now: number): void {
  const cue = tracker.observe(frame, matchId, eligible && get(soundStatus).enabled && get(soundSettings).eliminations, now);
  if (cue) audio.play(cue);
}
export function watchSoundVisibility(): () => void {
  const hide = () => { if (document.hidden) audio.mute(); };
  document.addEventListener('visibilitychange', hide);
  return () => { document.removeEventListener('visibilitychange', hide); audio.dispose(); };
}
