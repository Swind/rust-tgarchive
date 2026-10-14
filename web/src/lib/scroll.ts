export type Anchor = { id: number; offset: number };

export const isNearBottom = (scrollTop: number, clientHeight: number, scrollHeight: number, slack = 48) =>
  scrollHeight - scrollTop - clientHeight <= slack;

/** First message intersecting the viewport, with its top offset relative to the container top. */
export function pickAnchor(items: { id: number; top: number; bottom: number }[], containerTop: number): Anchor | null {
  const m = items.find((i) => i.bottom > containerTop);
  return m ? { id: m.id, offset: m.top - containerTop } : null;
}

/** New scrollTop that keeps an anchor at its remembered offset. */
export const restoreScrollTop = (scrollTop: number, savedOffset: number, currentOffset: number) =>
  scrollTop + currentOffset - savedOffset;

/** True when a newer message arrived (ids grow per chat); prepends and deleting the newest don't count. */
export const hasNewTail = (prevLastId: number | undefined, lastId: number | undefined) =>
  prevLastId !== undefined && lastId !== undefined && lastId > prevLastId;
