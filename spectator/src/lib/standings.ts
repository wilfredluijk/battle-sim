import type { TrainingSession } from '../types/protocol';
export interface Standing { name: string; place: number; points: number; played: number; wins: number; draws: number; losses: number; forfeits: number; damage: number; kills: number }
/** Training points only. Aborted/interrupted rounds are excluded; forfeits earn zero.
 * Exact team identities survive reconnects. Equal points share a place. */
export function standings(session: TrainingSession): Standing[] {
  const rows = new Map<string, Standing>();
  const row = (name: string) => {
    if (!rows.has(name)) rows.set(name, { name, place: 0, points: 0, played: 0, wins: 0, draws: 0, losses: 0, forfeits: 0, damage: 0, kills: 0 });
    return rows.get(name)!;
  };
  session.expected_teams.forEach(row);
  for (const round of session.rounds) {
    const report = round.report;
    if (round.status !== 'finished' || !report || !['winner', 'draw'].includes(report.outcome)) continue;
    for (const bot of report.bots) {
      const r = row(bot.name); r.played++; r.damage += bot.damage_dealt; r.kills += bot.kills;
      if (bot.forfeited) { r.forfeits++; continue; }
      if (report.outcome === 'draw') { r.draws++; r.points += session.scoring.draw; }
      else if (report.winner === bot.bot_id) { r.wins++; r.points += session.scoring.win; }
      else { r.losses++; r.points += session.scoring.loss; }
    }
  }
  const sorted = [...rows.values()].sort((a,b) => b.points - a.points || a.name.localeCompare(b.name));
  sorted.forEach((r,i) => { r.place = i && r.points === sorted[i-1].points ? sorted[i-1].place : i + 1; });
  return sorted;
}
export function csvCell(value: string | number): string {
  const text = String(value);
  return `"${(/^[=+@\-\t\r\n]/.test(text) ? "'" : '') + text.replace(/"/g, '""')}"`;
}
export function standingsCsv(session: TrainingSession): string {
  const fields: (keyof Standing)[] = ['place','name','points','played','wins','draws','losses','forfeits','damage','kills'];
  return [
    ['Session',session.name,'Training scoring',`win=${session.scoring.win}; draw=${session.scoring.draw}; loss=${session.scoring.loss}; forfeit=0; aborted/interrupted excluded; ties share place`].map(csvCell).join(','),
    fields.join(','), ...standings(session).map(r => fields.map(f => csvCell(r[f])).join(','))
  ].join('\r\n');
}
