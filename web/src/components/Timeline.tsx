import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api, type Message } from '../api/client';
import { ErrorBox, Empty } from './common';
import MessageItem from './MessageItem';
import { dayKey, formatDate } from '../lib/format';
import { groupPositions } from '../lib/bubble';
import { mergeMessages, nextOlderCursor } from '../lib/messages';
import { isNearBottom, hasNewTail, pickAnchor, restoreScrollTop, type Anchor } from '../lib/scroll';

export type TimelineFilters = { senderId?: number; toIso?: string; includeDeleted: boolean; excludeBots?: boolean };

function Rows({ messages, anchorId, chatTitle }: { messages: Message[]; anchorId?: number; chatTitle?: string | null }) {
  const pos = useMemo(() => groupPositions(messages), [messages]);
  const byId = useMemo(() => new Map(messages.map((m) => [m.id, m])), [messages]);
  return (
    <>
      {messages.map((m, i) => {
        const prev = messages[i - 1];
        const newDay = !prev || dayKey(prev.timestamp) !== dayKey(m.timestamp);
        return (
          <Fragment key={m.id}>
            {newDay && (
              <div className="date-sep">
                <span>{formatDate(m.timestamp)}</span>
              </div>
            )}
            <MessageItem
              message={m}
              anchor={m.id === anchorId}
              chatTitle={chatTitle}
              pos={pos[i]}
              replyTo={m.reply_to != null ? byId.get(m.reply_to) : undefined}
            />
          </Fragment>
        );
      })}
    </>
  );
}

/**
 * Scroll container. Anchors the first visible message across data changes; at the bottom it follows new
 * messages (when `follow`), otherwise it reports `hasNew` so the reader can opt in.
 */
function useScrollKeeper(messages: Message[], follow: boolean, anchorId?: number, layoutKey?: string) {
  const ref = useRef<HTMLDivElement>(null);
  const initialized = useRef(false);
  const atBottom = useRef(true);
  const anchor = useRef<Anchor | null>(null);
  const lastId = useRef<number | undefined>(undefined);
  const [hasNew, setHasNew] = useState(false);

  const snapshot = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    atBottom.current = isNearBottom(el.scrollTop, el.clientHeight, el.scrollHeight);
    const top = el.getBoundingClientRect().top;
    anchor.current = pickAnchor(
      [...el.querySelectorAll('[id^="m-"]')].map((n) => {
        const r = n.getBoundingClientRect();
        return { id: Number(n.id.slice(2)), top: r.top, bottom: r.bottom };
      }),
      top,
    );
    if (atBottom.current) setHasNew(false);
  }, []);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || messages.length === 0) return;
    const newLast = messages[messages.length - 1]!.id;
    if (!initialized.current) {
      initialized.current = true;
      const target = anchorId != null ? el.querySelector(`#m-${anchorId}`) : null;
      if (target) target.scrollIntoView({ block: 'center' });
      else el.scrollTop = el.scrollHeight;
    } else if (follow && atBottom.current) {
      el.scrollTop = el.scrollHeight;
    } else {
      const saved = anchor.current;
      const node = saved && el.querySelector(`#m-${saved.id}`);
      if (saved && node) {
        el.scrollTop = restoreScrollTop(el.scrollTop, saved.offset, node.getBoundingClientRect().top - el.getBoundingClientRect().top);
      }
      if (follow && hasNewTail(lastId.current, newLast)) setHasNew(true);
    }
    lastId.current = newLast;
    snapshot();
  }, [messages, follow, anchorId, layoutKey, snapshot]);

  const jumpToLatest = useCallback(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
    setHasNew(false);
  }, []);
  return { ref, onScroll: snapshot, hasNew, jumpToLatest };
}

function NewMessages({ onClick }: { onClick: () => void }) {
  return (
    <div className="new-messages" role="status">
      <button className="more" onClick={onClick}>
        有新訊息
      </button>
    </div>
  );
}

function TopSentinel({ onVisible, disabled }: { onVisible: () => void; disabled: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || disabled) return;
    const io = new IntersectionObserver((entries) => entries.some((e) => e.isIntersecting) && onVisible());
    io.observe(el);
    return () => io.disconnect();
  }, [onVisible, disabled]);
  return <div ref={ref} aria-hidden="true" style={{ height: 1 }} />;
}

export function ListTimeline({ chatId, filters, chatTitle }: { chatId: number; filters: TimelineFilters; chatTitle?: string | null }) {
  const q = useInfiniteQuery({
    queryKey: ['messages', chatId, filters],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      api.messages(chatId, {
        before: pageParam,
        limit: 50,
        sender_id: filters.senderId,
        to: filters.toIso,
        include_deleted: filters.includeDeleted || undefined,
        exclude_bots: filters.excludeBots || undefined,
      }),
    getNextPageParam: (last) => nextOlderCursor(last),
  });
  const messages = useMemo(() => mergeMessages(...(q.data?.pages.map((p) => p.items) ?? [])), [q.data]);
  const { ref, onScroll, hasNew, jumpToLatest } = useScrollKeeper(messages, !filters.toIso, undefined, `${q.hasNextPage}${q.isFetchingNextPage}`);
  const loadOlder = useCallback(() => {
    if (q.hasNextPage && !q.isFetching) void q.fetchNextPage();
  }, [q]);

  if (q.isPending) return <Empty>載入中…</Empty>;
  if (!q.data) return <ErrorBox error={q.error} />;
  return (
    <div className="timeline" ref={ref} onScroll={onScroll} tabIndex={0} aria-label="訊息時間軸">
      <TopSentinel onVisible={loadOlder} disabled={!q.hasNextPage || q.isFetching} />
      {q.isFetchingNextPage && <div className="muted center">載入較舊訊息…</div>}
      {!q.hasNextPage && messages.length > 0 && <div className="muted center">已到最早的訊息</div>}
      {messages.length === 0 ? <Empty>沒有符合的訊息</Empty> : <Rows messages={messages} chatTitle={chatTitle} />}
      <ErrorBox error={q.error} />
      {hasNew && <NewMessages onClick={jumpToLatest} />}
    </div>
  );
}

type Extra = { items: Message[]; more: boolean; cursor?: string | null; pages: number };

/** Fetches up to `pages` consecutive pages from `cursor`, in one direction. */
async function fetchChain(chatId: number, includeDeleted: boolean, dir: 'older' | 'newer', cursor: string | null | undefined, pages: number): Promise<Extra> {
  const items: Message[] = [];
  let more = true;
  let done = 0;
  while (more && cursor && done < pages) {
    const page = await api.messages(chatId, {
      [dir === 'older' ? 'before' : 'after']: cursor,
      limit: 50,
      include_deleted: includeDeleted || undefined,
    });
    items.push(...page.items);
    more = page.has_more;
    cursor = page.next_cursor;
    done++;
  }
  return { items, more, cursor, pages: done };
}

export function ContextTimeline({
  chatId,
  messageId,
  includeDeleted,
  chatTitle,
}: {
  chatId: number;
  messageId: number;
  includeDeleted: boolean;
  chatTitle?: string | null;
}) {
  const ctx = useQuery({
    queryKey: ['context', chatId, messageId, includeDeleted],
    queryFn: () => api.context(chatId, messageId, { before: 25, after: 25, include_deleted: includeDeleted || undefined }),
  });
  const [older, setOlder] = useState<Extra | null>(null);
  const [newer, setNewer] = useState<Extra | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>(null);
  const loaded = useRef({ older: 0, newer: 0 });
  loaded.current = { older: older?.pages ?? 0, newer: newer?.pages ?? 0 };

  const data = ctx.data;
  const messages = useMemo(
    () => (data ? mergeMessages(older?.items ?? [], data.before, [data.anchor], data.after, newer?.items ?? []) : []),
    [data, older, newer],
  );
  const { ref, onScroll } = useScrollKeeper(messages, false, messageId);

  // The context was refetched: re-read the manually loaded pages too so edits/deletes there don't go stale.
  useEffect(() => {
    const { older: no, newer: nn } = loaded.current;
    if (!data || ctx.isFetching || (!no && !nn)) return;
    let cancelled = false;
    setBusy(true);
    Promise.all([
      no ? fetchChain(chatId, includeDeleted, 'older', data.before_cursor, no) : null,
      nn ? fetchChain(chatId, includeDeleted, 'newer', data.after_cursor, nn) : null,
    ])
      .then(([o, n]) => {
        if (cancelled) return;
        setOlder(o);
        setNewer(n);
        setErr(null);
        setBusy(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setErr(e);
        setBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [data, ctx.isFetching, ctx.dataUpdatedAt, chatId, includeDeleted]);

  if (ctx.isPending) return <Empty>載入中…</Empty>;
  if (!data) return <ErrorBox error={ctx.error} />;

  const hasOlder = older ? older.more : data.has_more_before;
  const hasNewer = newer ? newer.more : data.has_more_after;
  const olderCursor = older ? older.cursor : data.before_cursor;
  const newerCursor = newer ? newer.cursor : data.after_cursor;

  const load = async (dir: 'older' | 'newer') => {
    setBusy(true);
    setErr(null);
    try {
      const prev = dir === 'older' ? older : newer;
      const page = await fetchChain(chatId, includeDeleted, dir, dir === 'older' ? olderCursor : newerCursor, 1);
      const next = { ...page, items: [...(prev?.items ?? []), ...page.items], pages: (prev?.pages ?? 0) + page.pages };
      if (dir === 'older') setOlder(next);
      else setNewer(next);
    } catch (e) {
      setErr(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="timeline" ref={ref} onScroll={onScroll} tabIndex={0} aria-label="訊息脈絡">
      {hasOlder && olderCursor && (
        <button className="more" disabled={busy} onClick={() => void load('older')}>
          載入更早的訊息
        </button>
      )}
      <Rows messages={messages} anchorId={data.anchor.id} chatTitle={chatTitle} />
      {hasNewer && newerCursor && (
        <button className="more" disabled={busy} onClick={() => void load('newer')}>
          載入較新的訊息
        </button>
      )}
      <ErrorBox error={err ?? ctx.error} />
    </div>
  );
}
