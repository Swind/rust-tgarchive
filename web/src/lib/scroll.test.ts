import { describe, expect, it } from 'vitest';
import { hasNewTail, isNearBottom, pickAnchor, restoreScrollTop } from './scroll';

describe('scroll helpers', () => {
  it('detects bottom', () => {
    expect(isNearBottom(900, 100, 1000)).toBe(true);
    expect(isNearBottom(500, 100, 1000)).toBe(false);
  });
  it('picks first visible message', () => {
    const items = [{ id: 1, top: -80, bottom: -10 }, { id: 2, top: -10, bottom: 40 }, { id: 3, top: 40, bottom: 90 }];
    expect(pickAnchor(items, 0)).toEqual({ id: 2, offset: -10 });
    expect(pickAnchor([], 0)).toBeNull();
  });
  it('keeps anchor when content is prepended', () => {
    expect(restoreScrollTop(100, 20, 320)).toBe(400);
  });
  it('flags only tail changes', () => {
    expect(hasNewTail(5, 6)).toBe(true);
    expect(hasNewTail(5, 5)).toBe(false);
    expect(hasNewTail(6, 5)).toBe(false);
    expect(hasNewTail(undefined, 5)).toBe(false);
  });
});
