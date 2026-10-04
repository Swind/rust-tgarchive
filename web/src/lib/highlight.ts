export type Segment = { text: string; match: boolean };

const isHan = (s: string) => /\p{Script=Han}/u.test(s);

/**
 * Terms to mark: the whitespace-separated chunks of the query plus, for Chinese text, its words
 * (the server matches Chinese by word and by character sequence, so "台北咖啡" also finds
 * "台北…咖啡"). Word splitting uses the browser's Intl.Segmenter when available.
 */
export function queryTerms(query: string): string[] {
  const terms = new Set(query.split(/\s+/).map((t) => t.replace(/^["']|["']$/g, '')).filter(Boolean));
  const Segmenter = (Intl as { Segmenter?: typeof Intl.Segmenter }).Segmenter;
  if (Segmenter && isHan(query)) {
    const segmenter = new Segmenter('zh', { granularity: 'word' });
    for (const { segment, isWordLike } of segmenter.segment(query)) {
      if (isWordLike && segment.length >= 2) terms.add(segment);
    }
  }
  return [...terms].sort((a, b) => b.length - a.length);
}

/** Splits `text` into segments, flagging case-insensitive occurrences of any query term. */
export function highlight(text: string, query: string): Segment[] {
  const terms = queryTerms(query);
  if (terms.length === 0) return [{ text, match: false }];
  const pattern = new RegExp(`(${terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi');
  return text
    .split(pattern)
    .map((part, i) => ({ text: part, match: i % 2 === 1 }))
    .filter((s) => s.text !== '');
}
