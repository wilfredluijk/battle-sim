import type { TickEvent } from '../types/protocol';
import { powerupLabel } from './powerupHud';
export type TimelineEvent = TickEvent | { type: 'disconnect'; ship_id: string; reason: string };
export function clockText(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
}
export function eventText(event: TimelineEvent, names: Map<string, string>): string {
  const name = 'ship_id' in event ? names.get(event.ship_id) ?? event.ship_id : '';
  switch (event.type) {
    case 'disconnect': return `${name} disconnected · ${event.reason}`;
    case 'hit': return `${name} hit · −${event.amount} HP`;
    case 'death': return `${name} destroyed`;
    case 'powerup_activated': return `${name} activated ${powerupLabel(event.powerup)}`;
    case 'shell_splash': return `Shell splash at ${event.pos.map(Math.round).join(', ')}`;
  }
}
export function download(name: string, contents: string, type = 'application/json'): void {
  const url = URL.createObjectURL(new Blob([contents], { type }));
  const a = document.createElement('a'); a.href = url; a.download = name; a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
