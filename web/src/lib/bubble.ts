import type { Message } from '../api/client';
import { dayKey } from './format';

export const GROUP_GAP_MS = 5 * 60 * 1000;
export const PALETTE_SIZE = 8;

const segmenter = typeof Intl !== 'undefined' && 'Segmenter' in Intl ? new Intl.Segmenter(undefined, { granularity: 'grapheme' }) : null;

function firstGrapheme(s: string): string {
  if (!s) return '';
  if (segmenter) {
    for (const part of segmenter.segment(s)) return part.segment;
  }
  return Array.from(s)[0] ?? '';
}

/** Up to two initials (first grapheme of the first two words), safe for CJK and emoji. */
export function initials(name: string | null | undefined): string {
  const words = (name ?? '').replace(/^@/, '').trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return '?';
  return words
    .slice(0, 2)
    .map((w) => firstGrapheme(w).toUpperCase())
    .join('');
}

/** Deterministic palette slot (0..PALETTE_SIZE-1) for a sender id. */
export function colorIndex(id: number | null | undefined): number {
  if (id == null) return 0;
  let h = Math.abs(id % 2147483647);
  h = Math.imul(h ^ (h >>> 15), 2246822519) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 3266489917) >>> 0;
  return ((h ^ (h >>> 16)) >>> 0) % PALETTE_SIZE;
}

export type GroupPos = { first: boolean; last: boolean };

type Groupable = Pick<Message, 'timestamp' | 'sender_id'> & { sender?: { id: number } | null };

const senderOf = (m: Groupable) => m.sender?.id ?? m.sender_id ?? null;

/** Same sender, same local day, ≤5 minutes apart. */
export function sameGroup(a: Groupable, b: Groupable): boolean {
  const gap = new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime();
  return senderOf(a) === senderOf(b) && dayKey(a.timestamp) === dayKey(b.timestamp) && Math.abs(gap) <= GROUP_GAP_MS;
}

export function groupPositions(messages: Groupable[]): GroupPos[] {
  return messages.map((m, i) => {
    const prev = messages[i - 1];
    const next = messages[i + 1];
    return { first: !prev || !sameGroup(prev, m), last: !next || !sameGroup(m, next) };
  });
}
