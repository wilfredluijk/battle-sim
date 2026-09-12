import { expect, it } from 'vitest';
import { standings, standingsCsv, csvCell } from '../lib/standings';
import type { BotReport, MatchReport, TrainingRound, TrainingSession } from '../types/protocol';
const bot = (name: string, bot_id: string, forfeited = false): BotReport => ({ name, bot_id, forfeited, shots_fired: 1, hits_landed: 2, accuracy: 2, damage_dealt: 10, damage_taken: 0, kills: 0, final_hp: 100, survived: true });
const round = (outcome: MatchReport['outcome'], bots: BotReport[], winner: string | null = null): TrainingRound => ({ match_id: 'test', name: 'Round', started_at: 0, config_hash: 'rules', teams: bots.map(b => b.name), status: 'finished', report: { room: 'main', replay_id: null, outcome, winner, winner_name: null, bots, duration_ticks: 1, duration_seconds: 0.1 } });
const session = (): TrainingSession => ({ id: 'test', name: 'Training', created_at: 0, expected_teams: ['Absent'], next_round: 'Next', scoring: { win: 3, draw: 1, loss: 0 }, rounds: [] });
it('accumulates exact team identities across reconnects and excludes aborted/interrupted rounds', () => {
  const s = session();
  s.rounds = [round('winner', [bot('Atlas','b1'), bot('Echo','b2')], 'b1'), round('draw', [bot('Atlas','b9'),bot('Echo','b7'),bot('Forfeit','b8',true)]), round('aborted',[bot('Atlas','b1')]), { ...round('winner',[bot('Echo','b2')],'b2'), status: 'interrupted' }];
  const rows = standings(s);
  expect(rows.find(r => r.name === 'Atlas')).toMatchObject({ points: 4, played: 2, wins: 1, draws: 1, damage: 20 });
  expect(rows.find(r => r.name === 'Echo')).toMatchObject({ points: 1, played: 2, losses: 1 });
  expect(rows.find(r => r.name === 'Forfeit')).toMatchObject({ points: 0, draws: 0, forfeits: 1 });
  expect(rows.find(r => r.name === 'Absent')).toMatchObject({ played: 0, points: 0 });
});
it('uses configured training points and shared places without damage tie-breakers', () => {
  const s = session(); s.scoring = { win: 5, draw: 2, loss: 1 };
  s.rounds = [round('draw',[bot('Atlas','a'),bot('Echo','b')])];
  expect(standings(s).map(r => [r.place,r.points])).toEqual([[1,2],[1,2],[3,0]]);
});
it('exports the scoring rule and neutralizes CSV formula prefixes and quoted team names', () => {
  const s = session(); s.name = '=HYPERLINK("bad")';
  expect(standingsCsv(s)).toContain('forfeit=0; aborted/interrupted excluded; ties share place');
  expect(standingsCsv(s)).toContain('"\'=HYPERLINK(""bad"")"');
  expect(csvCell('\t=1')).toBe('"\'\t=1"');
});
