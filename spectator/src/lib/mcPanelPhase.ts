// Which view the Monte Carlo panel shows. Pure so it can be unit-tested and so the
// component doesn't dead-end after a run finishes.
//
// The server keeps reporting the *last* run's status indefinitely (running:false,
// completed>0) until a new run starts. Deriving the phase purely from that status pins the
// panel to 'completed' forever, with no way to start a second batch. `showSetup` is a local
// override the "New run" button sets to force the setup form back up; it's ignored while a
// run is actually running so a late status frame can't strand the user on the form.

import type { McStatus } from '../types/protocol';

export type McPanelPhase = 'setup' | 'running' | 'completed';

export function mcPanelPhase(
  status: McStatus | null | undefined,
  showSetup: boolean,
): McPanelPhase {
  // A live run always wins — never show setup/completed over an in-flight batch.
  if (status?.running) return 'running';
  // Operator asked for a new run: show the setup form even though a prior run's status lingers.
  if (showSetup) return 'setup';
  // A finished run's status persists; show its results.
  if (status && status.completed > 0) return 'completed';
  return 'setup';
}
