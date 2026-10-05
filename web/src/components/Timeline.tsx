import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api, type Message } from '../api/client';
import { ErrorBox, Empty } from './common';
import MessageItem from './MessageItem';
import { dayKey, formatDate } from '../lib/format';
import { mergeMessages, nextOlderCursor } from '../lib/messages';

export type TimelineFilters = { senderId?: number; toIso?: string; includeDeleted: boolean; excludeBots?: boolean };

function Rows({ messages, anchorId, chatTitle }: { messages: Message[]; anchorId?: number; chatTitle?: string | null }) {
  let last = '';
  return (
    <>
      {messages.map((m) => {
        const key = dayKey(m.timestamp);
        const sep = key !== last;
        last = key;
        return (
          <div key={m.id}>
            {sep && <div className="date-sep">{formatDate(m.timestamp)}</div>}
            <MessageItem message={m} anchor={m.id === anchorId} chatTitle={chatTitle} />
          </div>
        );
      })}
    </>
  );
}

/** Scroll container that keeps position when content is prepended. */
function useScrollKeeper(count: number, anchorId?: number) {
  const ref = useRef<HTMLDivElement>(null);
  const prevHeight = useRef<number | null>(null);
  const initialized = useRef(false);
  const remember = useCallback(() => {
    prevHeight.current = ref.current?.scrollHeight ?? null;
  }, []);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || count === 0) return;
    if (!initialized.current) {
      initialized.current = true;
      const anchor = anchorId != null ? el.querySelector(`#m-${anchorId}`) : null;
      if (anchor) anchor.scrollIntoView({ block: 'center' });
      else el.scrollTop = el.scrollHeight;
    } else if (prevHeight.current != null) {
      el.scrollTop += el.scrollHeight - prevHeight.current;
    }
    prevHeight.current = null;
  }, [count, anchorId]);
  return { ref, remember };
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
  const { ref, remember } = useScrollKeeper(messages.length);
  const loadOlder = useCallback(() => {
    if (q.hasNextPage && !q.isFetchingNextPage) {
      remember();
      void q.fetchNextPage();
    }
  }, [q, remember]);

  if (q.isPending) return <Empty>載入中…</Empty>;
  if (q.error) return <ErrorBox error={q.error} />;
  return (
    <div className="timeline" ref={ref} tabIndex={0} aria-label="訊息時間軸">
      <TopSentinel onVisible={loadOlder} disabled={!q.hasNextPage} />
      {q.isFetchingNextPage && <div className="muted center">載入較舊訊息…</div>}
      {!q.hasNextPage && messages.length > 0 && <div className="muted center">已到最早的訊息</div>}
      {messages.length === 0 ? <Empty>沒有符合的訊息</Empty> : <Rows messages={messages} chatTitle={chatTitle} />}
    </div>
  );
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
  const [older, setOlder] = useState<{ items: Message[]; more: boolean; cursor?: string | null } | null>(null);
  const [newer, setNewer] = useState<{ items: Message[]; more: boolean; cursor?: string | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>(null);

  const data = ctx.data;
  const messages = useMemo(
    () => (data ? mergeMessages(older?.items ?? [], data.before, [data.anchor], data.after, newer?.items ?? []) : []),
    [data, older, newer],
  );
  const { ref, remember } = useScrollKeeper(messages.length, messageId);

  if (ctx.isPending) return <Empty>載入中…</Empty>;
  if (ctx.error) return <ErrorBox error={ctx.error} />;
  if (!data) return null;

  const hasOlder = older ? older.more : data.has_more_before;
  const hasNewer = newer ? newer.more : data.has_more_after;
  const olderCursor = older ? older.cursor : data.before_cursor;
  const newerCursor = newer ? newer.cursor : data.after_cursor;

  const load = async (dir: 'older' | 'newer') => {
    setBusy(true);
    setErr(null);
    try {
      if (dir === 'older') {
        remember();
        const page = await api.messages(chatId, { before: olderCursor, limit: 50, include_deleted: includeDeleted || undefined });
        setOlder({ items: [...(older?.items ?? []), ...page.items], more: page.has_more, cursor: page.next_cursor });
      } else {
        const page = await api.messages(chatId, { after: newerCursor, limit: 50, include_deleted: includeDeleted || undefined });
        setNewer({ items: [...(newer?.items ?? []), ...page.items], more: page.has_more, cursor: page.next_cursor });
      }
    } catch (e) {
      setErr(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="timeline" ref={ref} tabIndex={0} aria-label="訊息脈絡">
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
      <ErrorBox error={err} />
    </div>
  );
}
