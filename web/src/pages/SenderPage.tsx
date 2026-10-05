import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api, ApiError } from '../api/client';
import { Empty, ErrorBox, KIND_LABEL, useDebounced } from '../components/common';
import MessageItem, { Highlighted } from '../components/MessageItem';
import { KIND_SENDER, senderLabel } from '../components/senderFormat';
import { dayKey, formatDate, formatDateTime, formatRelative, localDayToIso } from '../lib/format';
import { nextOlderCursor } from '../lib/messages';
import { chatTitle } from './ChatsPage';

type Sort = 'relevance' | 'time';

function BottomSentinel({ onVisible, disabled }: { onVisible: () => void; disabled: boolean }) {
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

export default function SenderPage() {
  const { senderId } = useParams();
  const id = senderId && /^-?\d+$/.test(senderId) ? Number(senderId) : undefined;
  const [params, setParams] = useSearchParams();
  const chatId = params.get('chat') ?? '';
  const from = params.get('from') ?? '';
  const to = params.get('to') ?? '';
  const deleted = params.get('deleted') === '1';
  const q = (params.get('q') ?? '').trim();
  const sort: Sort = params.get('sort') === 'time' ? 'time' : 'relevance';
  const [text, setText] = useState(params.get('q') ?? '');
  const debounced = useDebounced(text.trim());

  const update = (changes: Record<string, string>, replace = false) =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        for (const [k, v] of Object.entries(changes)) {
          if (v) next.set(k, v);
          else next.delete(k);
        }
        return next;
      },
      { replace },
    );
  useEffect(() => {
    if (debounced !== q) update({ q: debounced }, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced]);
  useEffect(() => {
    setText((t) => (t.trim() === q ? t : q));
  }, [q]);

  const sender = useQuery({
    queryKey: ['sender', id, deleted],
    enabled: id !== undefined,
    queryFn: () => api.sender(id as number, deleted),
    retry: (count, error) => !(error instanceof ApiError && error.status === 404) && count < 2,
  });
  const chats = useQuery({ queryKey: ['chats', 'title'], queryFn: () => api.chats('title') });
  const titles = useMemo(() => new Map((chats.data ?? []).map((c) => [c.id, chatTitle(c)])), [chats.data]);

  const common = {
    sender_id: id,
    chat_id: chatId || undefined,
    from: localDayToIso(from, false),
    to: localDayToIso(to, true),
    include_deleted: deleted || undefined,
  };
  const timeline = useInfiniteQuery({
    queryKey: ['sender-messages', id, chatId, from, to, deleted],
    enabled: id !== undefined && q.length === 0,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => api.allMessages({ ...common, before: pageParam, limit: 50 }),
    getNextPageParam: nextOlderCursor,
  });
  const results = useInfiniteQuery({
    queryKey: ['sender-search', id, q, chatId, from, to, deleted, sort],
    enabled: id !== undefined && q.length > 0,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => api.search({ ...common, q, sort, before: pageParam, limit: 30 }),
    getNextPageParam: nextOlderCursor,
  });
  const active = q.length > 0 ? results : timeline;
  const items = active.data?.pages.flatMap((p) => p.items) ?? [];

  if (id === undefined) return <div className="page"><Empty>無效的使用者 ID</Empty></div>;
  if (sender.isPending) return <div className="page"><Empty>載入中…</Empty></div>;
  if (sender.error) {
    const notFound = sender.error instanceof ApiError && sender.error.status === 404;
    return (
      <div className="page">
        <p><Link to="/senders">← 使用者清單</Link></p>
        {notFound ? <Empty>找不到使用者 {id}</Empty> : <ErrorBox error={sender.error} />}
      </div>
    );
  }
  const s = sender.data;
  let lastDay = '';

  return (
    <div className="page">
      <p><Link to="/senders">← 使用者清單</Link></p>
      <header className="sender-head">
        <h2>
          {senderLabel(s)} {s.is_self && <span className="badge green">自己</span>}
          <span className="badge gray">{KIND_SENDER[s.kind]}</span>
        </h2>
        <div className="muted small">
          {s.username && <>@{s.username} · </>}ID {s.id}
        </div>
        <div className="sender-stats">
          <span>{s.message_count.toLocaleString()} 則訊息</span>
          <span>{s.chat_count} 個聊天室</span>
          {s.first_message_at && <span>最早 {formatDateTime(s.first_message_at)}</span>}
          {s.last_message_at && <span>最近 {formatRelative(s.last_message_at)}</span>}
        </div>
      </header>

      <section aria-label="聊天室分佈">
        <h3>聊天室分佈</h3>
        <ul className="sender-chats">
          {s.chats.map((c) => (
            <li key={c.chat_id}>
              <button
                type="button"
                aria-pressed={chatId === String(c.chat_id)}
                title={`${KIND_LABEL[c.kind]} · 最近 ${c.last_message_at ? formatDateTime(c.last_message_at) : '—'}`}
                onClick={() => update({ chat: chatId === String(c.chat_id) ? '' : String(c.chat_id) })}
              >
                {c.title || c.chat_id}（{c.message_count}）
              </button>
            </li>
          ))}
        </ul>
      </section>

      <div className="filters search-filters">
        <select aria-label="聊天室" value={chatId} onChange={(e) => update({ chat: e.target.value })}>
          <option value="">所有聊天室</option>
          {s.chats.map((c) => (
            <option key={c.chat_id} value={c.chat_id}>{c.title || c.chat_id}</option>
          ))}
        </select>
        <div className="date-range">
          <label>從 <input type="date" value={from} onChange={(e) => update({ from: e.target.value })} /></label>
          <label>到 <input type="date" value={to} onChange={(e) => update({ to: e.target.value })} /></label>
        </div>
        <label>
          <input type="checkbox" checked={deleted} onChange={(e) => update({ deleted: e.target.checked ? '1' : '' })} /> 顯示已刪除
        </label>
      </div>

      <input
        type="search"
        className="wide"
        aria-label="在此使用者的發言中搜尋"
        placeholder="在此使用者的發言中搜尋"
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      {q && (
        <div className="sort-toggle" role="group" aria-label="排序">
          <span className="muted small">排序</span>
          {([['relevance', '相關性'], ['time', '時間']] as [Sort, string][]).map(([value, label]) => (
            <button key={value} type="button" className="pill" aria-pressed={sort === value} onClick={() => update({ sort: value === 'time' ? 'time' : '' })}>
              {label}
            </button>
          ))}
        </div>
      )}

      <ErrorBox error={active.error} />
      {active.isPending && !active.error && <Empty>載入中…</Empty>}
      {active.data && items.length === 0 && <Empty>{q ? '找不到符合的訊息' : '沒有符合的訊息'}</Empty>}
      {q ? (
        <ul className="results" aria-label="搜尋結果">
          {items.map((m) => (
            <li key={`${m.chat_id}-${m.id}`}>
              <Link to={`/chats/${m.chat_id}?message=${m.id}${deleted ? '&deleted=1' : ''}`} className={`result${m.is_deleted ? ' deleted' : ''}`}>
                <div className="muted small">
                  <strong>{titles.get(m.chat_id) ?? m.chat_id}</strong> · {formatDateTime(m.timestamp)}
                  {m.is_deleted && <span className="badge red">已刪除</span>}
                </div>
                <div className="text"><Highlighted text={m.snippet ?? m.text ?? ''} query={q} /></div>
              </Link>
            </li>
          ))}
        </ul>
      ) : (
        <div className="sender-timeline" aria-label="發言時間軸">
          {items.map((m) => {
            const key = dayKey(m.timestamp);
            const sep = key !== lastDay;
            lastDay = key;
            return (
              <div key={`${m.chat_id}-${m.id}`}>
                {sep && <div className="date-sep">{formatDate(m.timestamp)}</div>}
                <MessageItem message={m} showChat={titles.get(m.chat_id) ?? String(m.chat_id)} chatTitle={titles.get(m.chat_id)} />
              </div>
            );
          })}
        </div>
      )}
      <BottomSentinel
        disabled={!active.hasNextPage || active.isFetchingNextPage}
        onVisible={() => void active.fetchNextPage()}
      />
      {active.isFetchingNextPage && <div className="muted center">載入更多…</div>}
      {!active.hasNextPage && items.length > 0 && <div className="muted center">沒有更多了</div>}
    </div>
  );
}
