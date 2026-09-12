<script lang="ts">
  import { soundSettings, soundStatus, updateSoundSettings, enableSounds, muteSounds, testSound } from '../stores/presenter';
  import type { PresenterCue } from '../lib/presenterAudio';
  let dialog: HTMLDialogElement;
  let testNotice = $state('');
  function test(cue: PresenterCue) {
    testNotice = testSound(cue) ? `${cue === 'countdown' ? 'Countdown' : 'Elimination'} test played. Check your speaker volume.` : 'Enable sounds and raise the volume to test.';
  }
</script>

<button class="topbar-btn" aria-pressed={$soundStatus.enabled} onclick={() => $soundStatus.enabled || $soundStatus.pending ? muteSounds() : enableSounds()}>
  {$soundStatus.pending ? 'Cancel sound setup' : $soundStatus.enabled ? 'Mute sounds' : 'Enable sounds'}
</button>
<button class="topbar-btn quiet" class:config-err={!!$soundStatus.error} onclick={() => dialog.showModal()}>Sound settings{$soundStatus.error ? ' •' : ''}</button>

<dialog class="sound-dialog" bind:this={dialog} aria-labelledby="sound-heading" onclose={() => testNotice = ''}>
  <div class="sound-dialog-head"><h2 id="sound-heading">Presenter sounds</h2><button class="topbar-btn" onclick={() => dialog.close()} aria-label="Close sound settings">Close</button></div>
  <p class="pm-sub">Optional cues for the countdown and ships destroyed in the live view. Multiple ships destroyed in one tick make one cue.</p>
  <p role="status"><strong>{$soundStatus.pending ? 'Starting audio…' : $soundStatus.enabled ? 'Sounds enabled' : 'Muted'}</strong></p>
  {#if $soundStatus.error}<p class="config-err" role="alert">{$soundStatus.error}</p>{/if}
  <label class="sound-volume" for="presenter-volume">Volume <output for="presenter-volume">{Math.round($soundSettings.volume * 100)}%</output>
    <input id="presenter-volume" type="range" min="0" max="100" step="5" value={$soundSettings.volume * 100} oninput={e => updateSoundSettings({ volume: Number(e.currentTarget.value) / 100 })} />
  </label>
  <label class="sound-option"><input type="checkbox" checked={$soundSettings.countdown} onchange={e => updateSoundSettings({ countdown: e.currentTarget.checked })} /> Countdown tones</label>
  <label class="sound-option"><input type="checkbox" checked={$soundSettings.eliminations} onchange={e => updateSoundSettings({ eliminations: e.currentTarget.checked })} /> Ship elimination tones</label>
  <div class="sound-actions">
    <button class="topbar-btn" onclick={() => $soundStatus.enabled || $soundStatus.pending ? muteSounds() : enableSounds()}>{$soundStatus.pending ? 'Cancel sound setup' : $soundStatus.enabled ? 'Mute sounds' : 'Enable sounds'}</button>
    <button class="topbar-btn" disabled={!$soundStatus.enabled || !$soundSettings.volume} onclick={() => test('countdown')}>Test countdown</button>
    <button class="topbar-btn" disabled={!$soundStatus.enabled || !$soundSettings.volume} onclick={() => test('elimination')}>Test elimination</button>
  </div>
  {#if testNotice}<p class="pm-sub" role="status">{testNotice}</p>{/if}
  <p class="config-note">Volume and cue choices are saved in this browser. Sounds start muted on reload and mute when you leave this tab or sign out. Replays and background matches stay quiet. Enable sounds only on the presenting device.</p>
</dialog>
