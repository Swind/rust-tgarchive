import { describe, expect, it } from 'vitest';
import { colorIndex, groupPositions, initials, PALETTE_SIZE } from './bubble';

const m = (sender_id: number | null, timestamp: string) => ({ sender_id, timestamp });

describe('initials', () => {
  it('handles latin, CJK, emoji and empty names', () => {
    expect(initials('alice smith jr')).toBe('AS');
    expect(initials('Alice')).toBe('A');
    expect(initials('愛麗絲')).toBe('愛');
    expect(initials('Alice 愛麗絲')).toBe('A愛');
    expect(initials('👩‍👩‍👧 Family')).toBe('👩‍👩‍👧F');
    expect(initials('@bob')).toBe('B');
    expect(initials('')).toBe('?');
    expect(initials(null)).toBe('?');
  });
});

describe('colorIndex', () => {
  it('is deterministic and within the palette', () => {
    for (const id of [0, 1, 42, -1002000001, 9007199254740991]) {
      expect(colorIndex(id)).toBe(colorIndex(id));
      expect(colorIndex(id)).toBeGreaterThanOrEqual(0);
      expect(colorIndex(id)).toBeLessThan(PALETTE_SIZE);
    }
    expect(new Set([1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(colorIndex)).size).toBeGreaterThan(3);
  });
});

describe('groupPositions', () => {
  it('groups same sender within 5 minutes on the same day', () => {
    const list = [
      m(1, '2026-03-01T10:00:00'),
      m(1, '2026-03-01T10:04:59'),
      m(1, '2026-03-01T10:12:00'),
      m(2, '2026-03-01T10:12:30'),
      m(2, '2026-03-02T00:00:10'),
    ];
    expect(groupPositions(list)).toEqual([
      { first: true, last: false },
      { first: false, last: true },
      { first: true, last: true },
      { first: true, last: true },
      { first: true, last: true },
    ]);
  });
  it('handles empty input', () => expect(groupPositions([])).toEqual([]));
});
