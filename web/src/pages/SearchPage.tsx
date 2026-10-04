import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import { Empty, ErrorBox, useDebounced } from '../components/common';
import { Highlighted, senderName } from '../components/MessageItem';
import { formatDateTime, localDayToIso } from '../lib/format';
import { nextOlderCursor } from '../lib/messages';
import { chatTitle } from './ChatsPage';

const MIN_LEN = 2;

export default function SearchPage() {
  const [text, setText] = useState('');
  const [chatId, setChatId] = useState('');
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [deleted, setDeleted] = useState(false);
  const q = useDebounced(text.trim());
  const enabled = q.length >= MIN_LEN;

  const chats = useQuery({ queryKey: ['chats', 'title'], queryFn: () => api.chats('title') });
  const titles = useMemo(() => new Map((chats.data ?? []).map((c) => [c.id, chatTitle(c)])), [chats.data]);

  const results = useInfiniteQuery({
    queryKey: ['search', q, chatId, from, to, deleted],
    enabled,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      api.search({
        q,
        chat_id: chatId || undefined,
        from: localDayToIso(from, false),
        to: localDayToIso(to, true),
        include_deleted: deleted || undefined,
        before: pageParam,
        limit: 30,
      }),
    getNextPageParam: nextOlderCursor,
  });
  const items = results.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <div className="page">
      <h2>搜尋</h2>
      <input
        type="search"
        autoFocus
        className="wide"
        aria-label="搜尋關鍵字"
        placeholder={`輸入關鍵字（至少 ${MIN_LEN} 字）`}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <div className="filters search-filters">
        <select aria-label="聊天室" value={chatId} onChange={(e) => setChatId(e.target.value)}>
          <option value="">所有聊天室</option>
          {chats.data?.map((c) => (
            <option key={c.id} value={c.id}>{chatTitle(c)}</option>
          ))}
        </select>
        <div className="date-range">
          <label>從 <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} /></label>
          <label>到 <input type="date" value={to} onChange={(e) => setTo(e.target.value)} /></label>
        </div>
        <label><input type="checkbox" checked={deleted} onChange={(e) => setDeleted(e.target.checked)} /> 包含已刪除</label>
      </div>
      <p className="muted small">
        注意：全文檢索採 SQLite FTS5 unicode61，以「完整詞元」比對；中文沒有斷詞，連續中文字串會被視為一個詞元，因此只能比對整段（或以空白分隔的）詞，無法搜尋詞中的部分字。
      </p>
      {!enabled && <Empty>{q ? `請至少輸入 ${MIN_LEN} 個字元` : '輸入關鍵字開始搜尋'}</Empty>}
      <ErrorBox error={results.error} />
      {enabled && results.isPending && !results.error && <Empty>搜尋中…</Empty>}
      {enabled && results.data && items.length === 0 && <Empty>找不到符合的訊息</Empty>}
      <ul className="results">
        {items.map((m) => (
          <li key={`${m.chat_id}-${m.id}`}>
            <Link to={`/chats/${m.chat_id}?message=${m.id}${deleted ? '&deleted=1' : ''}`} className={`result${m.is_deleted ? ' deleted' : ''}`}>
              <div className="muted small">
                <strong>{titles.get(m.chat_id) ?? m.chat_id}</strong> · {senderName(m, titles.get(m.chat_id))} · {formatDateTime(m.timestamp)}
                {m.is_deleted && <span className="badge red">已刪除</span>}
              </div>
              <div className="text"><Highlighted text={m.text ?? ''} query={q} /></div>
            </Link>
          </li>
        ))}
      </ul>
      {results.hasNextPage && (
        <button disabled={results.isFetchingNextPage} onClick={() => void results.fetchNextPage()}>
          {results.isFetchingNextPage ? '載入中…' : '載入更多'}
        </button>
      )}
    </div>
  );
}
