import type { McStatus } from '../types/protocol';
export function monteCarloPhase(status: McStatus | null, showSetup: boolean): 'setup' | 'running' | 'completed' {
  if (status?.running) return 'running';
  return status && status.completed > 0 && !showSetup ? 'completed' : 'setup';
}
