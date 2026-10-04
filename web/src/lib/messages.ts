import type { Message } from '../api/client';

/** Merges message lists, de-duplicating by id and sorting oldest first. */
export function mergeMessages(...lists: Message[][]): Message[] {
  const byId = new Map<number, Message>();
  for (const list of lists) for (const m of list) byId.set(m.id, m);
  return [...byId.values()].sort(
    (a, b) => Date.parse(a.timestamp) - Date.parse(b.timestamp) || a.id - b.id,
  );
}

/** Cursor of the oldest page that still has more history, or undefined when exhausted. */
export function nextOlderCursor(page: { has_more: boolean; next_cursor?: string | null }): string | undefined {
  return page.has_more && page.next_cursor ? page.next_cursor : undefined;
}
