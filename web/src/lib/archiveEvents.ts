import type { QueryClient, QueryKey } from '@tanstack/react-query';

export type ArchiveChange = { chatIds: Set<number>; global: boolean };

// Keys scoped to one chat carry the chat id at index 1.
const CHAT_SCOPED = new Set(['chat', 'senders', 'messages', 'context']);
// Archive-derived keys that are not chat-scoped (or filter by chat inside params): refresh on any change.
const ARCHIVE_WIDE = new Set([
  'chats', 'search', 'senders-list', 'senders-pick', 'sender', 'sender-messages', 'sender-search',
]);

export function isArchiveQuery(key: QueryKey, change: ArchiveChange): boolean {
  const head = key[0];
  if (typeof head !== 'string') return false;
  if (CHAT_SCOPED.has(head)) return change.global || (typeof key[1] === 'number' && change.chatIds.has(key[1]));
  return ARCHIVE_WIDE.has(head);
}

/** Marks related queries stale; only active ones refetch. */
export async function invalidateArchive(qc: QueryClient, change: ArchiveChange) {
  const predicate = (q: { queryKey: QueryKey }) => isArchiveQuery(q.queryKey, change);
  // An initial fetch (no data yet) is joined, not restarted, so it may predate this change: refresh again once it settles.
  const initialInFlight = qc.getQueryCache().findAll({ predicate }).some((q) => q.state.data === undefined && q.state.fetchStatus === 'fetching');
  await qc.invalidateQueries({ predicate });
  if (initialInFlight) await qc.invalidateQueries({ predicate });
}

type Source = {
  addEventListener(type: string, fn: (e: MessageEvent) => void): void;
  close(): void;
};
type Deps = {
  createSource?: (url: string) => Source;
  doc?: Pick<Document, 'addEventListener' | 'removeEventListener' | 'visibilityState'>;
  delayMs?: number;
};

/** One EventSource per call; the browser reconnects, and the server sends `resync` on each connection. */
export function connectArchiveEvents(qc: QueryClient, deps: Deps = {}): () => void {
  const createSource = deps.createSource ?? ((u: string) => new EventSource(u));
  const doc = deps.doc ?? (typeof document === 'undefined' ? undefined : document);
  const delay = deps.delayMs ?? 300;
  let pending: ArchiveChange = { chatIds: new Set(), global: false };
  let timer: ReturnType<typeof setTimeout> | undefined;

  const schedule = () => {
    timer ??= setTimeout(() => {
      timer = undefined;
      const change = pending;
      pending = { chatIds: new Set(), global: false };
      void invalidateArchive(qc, change);
    }, delay);
  };
  const markGlobal = () => {
    pending.global = true;
    schedule();
  };

  const es = createSource('/api/v1/events');
  es.addEventListener('resync', markGlobal);
  es.addEventListener('archive_changed', (e) => {
    try {
      const d = JSON.parse(e.data) as { chat_ids?: unknown; global?: unknown };
      if (d.global === true) pending.global = true;
      if (Array.isArray(d.chat_ids)) for (const id of d.chat_ids) if (typeof id === 'number') pending.chatIds.add(id);
    } catch {
      pending.global = true;
    }
    schedule();
  });
  const onVisible = () => doc?.visibilityState === 'visible' && markGlobal();
  doc?.addEventListener('visibilitychange', onVisible);

  return () => {
    es.close();
    doc?.removeEventListener('visibilitychange', onVisible);
    clearTimeout(timer);
  };
}
