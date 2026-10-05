import { useEffect, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { useInfiniteQuery } from '@tanstack/react-query';
import { api, type SenderSort } from '../api/client';
import BotBadge from '../components/BotBadge';
import { Empty, ErrorBox, useDebounced } from '../components/common';
import { KIND_SENDER, senderLabel } from '../components/senderFormat';
import { formatRelative } from '../lib/format';

const SORTS: [SenderSort, string][] = [
  ['messages', '訊息數'],
  ['last_message', '最近發言'],
  ['name', '名稱'],
];

type BotFilter = 'all' | 'human' | 'bot';
const BOT_FILTERS: [BotFilter, string][] = [
  ['all', '全部'],
  ['human', '人'],
  ['bot', 'Bot'],
];

export default function SendersPage() {
  const [params, setParams] = useSearchParams();
  const q = (params.get('q') ?? '').trim();
  const sortParam = params.get('sort');
  const sort: SenderSort = sortParam === 'last_message' || sortParam === 'name' ? sortParam : 'messages';
  const botParam = params.get('bot');
  const bot: BotFilter = botParam === 'human' || botParam === 'bot' ? botParam : 'all';
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

  const list = useInfiniteQuery({
    queryKey: ['senders-list', q, sort, bot],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => api.senderList({ q, sort, is_bot: bot === 'all' ? undefined : bot === 'bot', limit: 100, cursor: pageParam }),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  const items = list.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <div className="page">
      <h2>使用者</h2>
      <input
        type="search"
        className="wide"
        aria-label="搜尋使用者"
        placeholder="名稱、@使用者名稱或 ID（中文可搜尋部分字）"
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <div className="sort-toggle" role="group" aria-label="排序">
        <span className="muted small">排序</span>
        {SORTS.map(([value, label]) => (
          <button
            key={value}
            type="button"
            className="pill"
            aria-pressed={sort === value}
            onClick={() => update({ sort: value === 'messages' ? '' : value })}
          >
            {label}
          </button>
        ))}
      </div>
      <div className="sort-toggle" role="group" aria-label="類型">
        <span className="muted small">類型</span>
        {BOT_FILTERS.map(([value, label]) => (
          <button
            key={value}
            type="button"
            className="pill"
            aria-pressed={bot === value}
            onClick={() => update({ bot: value === 'all' ? '' : value })}
          >
            {label}
          </button>
        ))}
      </div>
      <ErrorBox error={list.error} />
      {list.isPending && !list.error && <Empty>載入中…</Empty>}
      {list.data && items.length === 0 && <Empty>找不到符合的使用者</Empty>}
      <ul className="results" aria-label="使用者清單">
        {items.map((s) => (
          <li key={s.id} className="sender-row">
            <div className="sender-main">
              <Link className="sender-link" to={`/senders/${s.id}`}>
                <strong>{senderLabel(s)}</strong>
              </Link>{' '}
              <BotBadge isBot={s.is_bot} />
              {s.is_self && <span className="badge green">自己</span>}
              <span className="badge gray">{KIND_SENDER[s.kind]}</span>
              <div className="muted small">
                {s.username && <>@{s.username} · </>}ID {s.id}
              </div>
              {s.matched_history && s.matched_name && (
                <div className="muted small history-hint">
                  曾用名：{s.matched_name.display_name || '（無名稱）'}
                  {s.matched_name.username && <> @{s.matched_name.username}</>}
                </div>
              )}
            </div>
            <span className="num" title="訊息數">{s.message_count.toLocaleString()} 則</span>
            <span className="num" title="聊天室數">{s.chat_count} 個聊天室</span>
            <span className="muted small" title="最近發言">{formatRelative(s.last_message_at)}</span>
          </li>
        ))}
      </ul>
      {list.hasNextPage && (
        <button disabled={list.isFetchingNextPage} onClick={() => void list.fetchNextPage()}>
          {list.isFetchingNextPage ? '載入中…' : '載入更多'}
        </button>
      )}
    </div>
  );
}
