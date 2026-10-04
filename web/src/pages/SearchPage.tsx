import { useEffect, useMemo, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import { Empty, ErrorBox, useDebounced } from '../components/common';
import { Highlighted, senderName } from '../components/MessageItem';
import { formatDateTime, localDayToIso } from '../lib/format';
import { nextOlderCursor } from '../lib/messages';
import { chatTitle } from './ChatsPage';

type Sort = 'relevance' | 'time';
const SORTS: [Sort, string][] = [['relevance', '相關性'], ['time', '時間']];

export default function SearchPage() {
  const [params, setParams] = useSearchParams();
  const q = (params.get('q') ?? '').trim();
  const chatId = params.get('chat') ?? '';
  const from = params.get('from') ?? '';
  const to = params.get('to') ?? '';
  const deleted = params.get('include_deleted') === '1';
  const [deletedBox, setDeletedBox] = useState(deleted);
  useEffect(() => setDeletedBox(deleted), [deleted]);
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

  // Typing -> URL (replace, debounced); URL changes from back/forward -> input.
  useEffect(() => {
    if (debounced !== q) update({ q: debounced }, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced]);
  useEffect(() => {
    setText((t) => (t.trim() === q ? t : q));
  }, [q]);
  const enabled = q.length > 0;

  const chats = useQuery({ queryKey: ['chats', 'title'], queryFn: () => api.chats('title') });
  const titles = useMemo(() => new Map((chats.data ?? []).map((c) => [c.id, chatTitle(c)])), [chats.data]);

  const results = useInfiniteQuery({
    queryKey: ['search', q, chatId, from, to, deleted, sort],
    enabled,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      api.search({
        q,
        chat_id: chatId || undefined,
        from: localDayToIso(from, false),
        to: localDayToIso(to, true),
        include_deleted: deleted || undefined,
        sort,
        before: pageParam,
        limit: 30,
      }),
    getNextPageParam: nextOlderCursor,
  });
  const items = results.data?.pages.flatMap((p) => p.items) ?? [];
  const status = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 15000 });
  const index = status.data?.search_index;
  const singleChar = [...q].length === 1 && /\p{Script=Han}/u.test(q);

  return (
    <div className="page">
      <h2>搜尋</h2>
      <input
        type="search"
        autoFocus
        className="wide"
        aria-label="搜尋關鍵字"
        placeholder="輸入關鍵字（中文可搜尋詞中的部分字）"
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <div className="filters search-filters">
        <select aria-label="聊天室" value={chatId} onChange={(e) => update({ chat: e.target.value })}>
          <option value="">所有聊天室</option>
          {chats.data?.map((c) => (
            <option key={c.id} value={c.id}>{chatTitle(c)}</option>
          ))}
        </select>
        <div className="date-range">
          <label>從 <input type="date" value={from} onChange={(e) => update({ from: e.target.value })} /></label>
          <label>到 <input type="date" value={to} onChange={(e) => update({ to: e.target.value })} /></label>
        </div>
        <label><input type="checkbox" checked={deletedBox} onChange={(e) => {
          setDeletedBox(e.target.checked);
          update({ include_deleted: e.target.checked ? '1' : '' });
        }} /> 包含已刪除</label>
      </div>
      <div className="sort-toggle" role="group" aria-label="排序">
        <span className="muted small">排序</span>
        {SORTS.map(([value, label]) => (
          <button key={value} type="button" className="pill" aria-pressed={sort === value} onClick={() => update({ sort: value === 'time' ? 'time' : '' })}>
            {label}
          </button>
        ))}
      </div>
      {index && index.state !== 'ready' && (
        <div className="banner" role="status">
          搜尋索引{index.state === 'rebuilding' ? '重建中' : '尚未建立或已過期'}（{index.indexed.toLocaleString()}／{index.total.toLocaleString()}）：
          暫時改用較慢的子字串掃描，結果依時間排序。請執行 <code>tgarchive search rebuild-index</code>。
        </div>
      )}
      {singleChar && (
        <div className="banner" role="status">單字查詢會逐筆掃描訊息文字（較慢），結果依時間排序。</div>
      )}
      <p className="muted small">
        全文檢索支援中文：以結巴斷詞比對詞彙，並以相鄰字元（bigram）比對任意中文子字串，例如「北咖啡」可找到「新北咖啡店」。
        預設依相關性排序，可切換為時間排序；英文以單字前綴比對（3 字元以上，例如 benchmark 可找到 benchmarks），不分大小寫與全形／半形。
      </p>
      {!enabled && <Empty>輸入關鍵字開始搜尋</Empty>}
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
              <div className="text"><Highlighted text={m.snippet ?? m.text ?? ''} query={q} /></div>
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
