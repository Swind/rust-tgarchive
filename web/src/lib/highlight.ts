export type Segment = { text: string; match: boolean };

/** Splits `text` into segments, flagging case-insensitive occurrences of any query term. */
export function highlight(text: string, query: string): Segment[] {
  const terms = [...new Set(query.split(/\s+/).map((t) => t.replace(/^["']|["']$/g, '')).filter(Boolean))]
    .filter((t) => !/^(AND|OR|NOT|NEAR)$/.test(t))
    .sort((a, b) => b.length - a.length);
  if (terms.length === 0) return [{ text, match: false }];
  const pattern = new RegExp(`(${terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi');
  return text
    .split(pattern)
    .map((part, i) => ({ text: part, match: i % 2 === 1 }))
    .filter((s) => s.text !== '');
}
