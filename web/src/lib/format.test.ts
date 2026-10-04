import { describe, expect, it } from 'vitest';
import { formatRelative, formatSize, localDayToIso } from './format';
import { highlight } from './highlight';
import { mergeMessages, nextOlderCursor } from './messages';
import { buildQuery } from '../api/client';

const msg = (id: number, timestamp: string) => ({
  id, chat_id: 1, timestamp, collected_at: timestamp, attachments: [], is_deleted: false,
});

describe('formatSize', () => {
  it('formats units', () => {
    expect(formatSize(0)).toBe('0 B');
    expect(formatSize(1536)).toBe('1.5 KB');
    expect(formatSize(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(formatSize(null)).toBe('');
  });
});

describe('dates', () => {
  it('relative', () => {
    const now = Date.parse('2026-01-01T00:00:00Z');
    expect(formatRelative(null, now)).toBe('—');
    expect(formatRelative('2025-12-31T23:00:00Z', now)).toContain('1');
  });
  it('local day to iso', () => {
    expect(localDayToIso('bad', false)).toBeUndefined();
    const start = Date.parse(localDayToIso('2026-03-05', false)!);
    const end = Date.parse(localDayToIso('2026-03-05', true)!);
    expect(end - start).toBe(86400_000 - 1);
  });
});

describe('highlight', () => {
  it('marks terms case-insensitively and escapes regex', () => {
    expect(highlight('Hello world', 'hello')).toEqual([
      { text: 'Hello', match: true },
      { text: ' world', match: false },
    ]);
    expect(highlight('a.b', '.')).toEqual([
      { text: 'a', match: false }, { text: '.', match: true }, { text: 'b', match: false },
    ]);
    expect(highlight('x', '')).toEqual([{ text: 'x', match: false }]);
  });
  it('marks single characters and words of a Chinese query', () => {
    expect(highlight('台北', '北')).toEqual([
      { text: '台', match: false }, { text: '北', match: true },
    ]);
    const marked = highlight('今天去台北喝咖啡', '台北咖啡').filter((s) => s.match).map((s) => s.text);
    expect(marked).toEqual(['台北', '咖啡']);
  });
});

describe('messages', () => {
  it('merges, dedupes and sorts', () => {
    const merged = mergeMessages([msg(2, '2026-01-02T00:00:00Z')], [msg(1, '2026-01-01T00:00:00Z'), msg(2, '2026-01-02T00:00:00Z')]);
    expect(merged.map((m) => m.id)).toEqual([1, 2]);
  });
  it('cursor', () => {
    expect(nextOlderCursor({ has_more: true, next_cursor: 'c' })).toBe('c');
    expect(nextOlderCursor({ has_more: false, next_cursor: 'c' })).toBeUndefined();
  });
  it('buildQuery skips empties', () => {
    expect(buildQuery({ a: 1, b: undefined, c: '', d: false })).toBe('?a=1&d=false');
    expect(buildQuery({})).toBe('');
  });
});
