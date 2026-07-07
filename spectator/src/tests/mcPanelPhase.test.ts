import { describe, expect, it } from 'vitest';
import { mcPanelPhase } from '../lib/mcPanelPhase';
import type { McStatus } from '../types/protocol';

function status(partial: Partial<McStatus>): McStatus {
  return {
    running: false,
    run_id: 'r',
    completed: 0,
    total: 0,
    variance_mode: 'shuffled',
    mc_seed: 0,
    started_at_unix: 0,
    finished_at_unix: null,
    current_match_tick: 0,
    wins: {},
    bot_names: {},
    draws: 0,
    results: [],
    ended_reason: null,
    ...partial,
  };
}

describe('mcPanelPhase', () => {
  it('is setup when there is no status', () => {
    expect(mcPanelPhase(null, false)).toBe('setup');
    expect(mcPanelPhase(undefined, false)).toBe('setup');
  });

  it('is running while a batch is in flight', () => {
    expect(mcPanelPhase(status({ running: true, completed: 3 }), false)).toBe('running');
  });

  it('is completed after a run finishes', () => {
    expect(mcPanelPhase(status({ running: false, completed: 10 }), false)).toBe('completed');
  });

  it('returns to setup when "New run" is requested even though a completed run lingers', () => {
    // The regression: without showSetup this pinned to "completed" forever.
    expect(mcPanelPhase(status({ running: false, completed: 10 }), true)).toBe('setup');
  });

  it('never shows setup over an actually-running batch, even if showSetup is set', () => {
    expect(mcPanelPhase(status({ running: true, completed: 2 }), true)).toBe('running');
  });
});
