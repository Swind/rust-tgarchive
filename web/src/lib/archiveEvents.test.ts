import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { connectArchiveEvents, invalidateArchive, isArchiveQuery } from './archiveEvents';

const change = (ids: number[], global = false) => ({ chatIds: new Set(ids), global });

describe('isArchiveQuery', () => {
  it('targets chat-scoped keys by id', () => {
    expect(isArchiveQuery(['messages', 1, {}], change([1]))).toBe(true);
    expect(isArchiveQuery(['messages', 2, {}], change([1]))).toBe(false);
    expect(isArchiveQuery(['context', 2, 5, false], change([], true))).toBe(true);
    expect(isArchiveQuery(['chat', 1], change([1]))).toBe(true);
    expect(isArchiveQuery(['senders', 3], change([1]))).toBe(false);
  });
  it('refreshes archive-wide keys and ignores polled/media keys', () => {
    for (const k of ['chats', 'search', 'senders-list', 'sender', 'sender-messages']) expect(isArchiveQuery([k, 9], change([1]))).toBe(true);
    for (const k of ['status', 'sync-status', 'media-progress', 'message-media']) expect(isArchiveQuery([k, 1], change([1], true))).toBe(false);
  });
});

describe('connectArchiveEvents', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function setup() {
    const qc = new QueryClient();
    const spy = vi.spyOn(qc, 'invalidateQueries').mockResolvedValue();
    const handlers: Partial<Record<string, (e: MessageEvent) => void>> = {};
    const close = vi.fn();
    const urls: string[] = [];
    const docHandlers: Partial<Record<string, () => void>> = {};
    const doc = {
      visibilityState: 'visible' as DocumentVisibilityState,
      addEventListener: vi.fn((t: string, f: () => void) => (docHandlers[t] = f)),
      removeEventListener: vi.fn(),
    };
    const stop = connectArchiveEvents(qc, {
      createSource: (u) => (urls.push(u), { addEventListener: (t, f) => (handlers[t] = f), close }),
      doc: doc as never,
    });
    return { spy, handlers, close, urls, doc, docHandlers, stop };
  }
  const ev = (d: unknown) => ({ data: JSON.stringify(d) }) as MessageEvent;

  it('opens one source, coalesces events and merges chat ids', () => {
    const s = setup();
    expect(s.urls).toEqual(['/api/v1/events']);
    s.handlers.archive_changed!(ev({ chat_ids: [1], global: false }));
    s.handlers.archive_changed!(ev({ chat_ids: [2], global: false }));
    vi.advanceTimersByTime(299);
    expect(s.spy).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(s.spy).toHaveBeenCalledTimes(1);
    const pred = s.spy.mock.calls[0]![0]!.predicate!;
    const q = (key: unknown[]) => ({ queryKey: key }) as never;
    expect(pred(q(['messages', 1]))).toBe(true);
    expect(pred(q(['messages', 2]))).toBe(true);
    expect(pred(q(['messages', 3]))).toBe(false);
  });

  it('resync and visibility invalidate globally', () => {
    const s = setup();
    s.handlers.resync!(ev({}));
    vi.advanceTimersByTime(300);
    expect(s.spy.mock.calls[0]![0]!.predicate!({ queryKey: ['messages', 99] } as never)).toBe(true);
    s.doc.visibilityState = 'hidden';
    s.docHandlers.visibilitychange!();
    vi.advanceTimersByTime(300);
    expect(s.spy).toHaveBeenCalledTimes(1);
    s.doc.visibilityState = 'visible';
    s.docHandlers.visibilitychange!();
    vi.advanceTimersByTime(300);
    expect(s.spy).toHaveBeenCalledTimes(2);
  });

  it('closes and cancels pending work on cleanup', () => {
    const s = setup();
    s.handlers.archive_changed!(ev({ chat_ids: [1], global: false }));
    s.stop();
    vi.advanceTimersByTime(1000);
    expect(s.close).toHaveBeenCalled();
    expect(s.doc.removeEventListener).toHaveBeenCalled();
    expect(s.spy).not.toHaveBeenCalled();
  });
});

describe('invalidateArchive', () => {
  it('refetches again after an initial fetch that raced the change', async () => {
    vi.useRealTimers();
    const qc = new QueryClient();
    let calls = 0;
    const obs = new QueryObserver(qc, { queryKey: ['messages', 1], queryFn: async () => ++calls });
    const unsub = obs.subscribe(() => {});
    await invalidateArchive(qc, { chatIds: new Set([1]), global: false });
    unsub();
    expect(calls).toBe(2);
  });
});
