import { useMemo, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, type Chat, type ChatKind, type ChatSort } from '../api/client';
import { Empty, ErrorBox, KIND_ICON, KIND_LABEL } from '../components/common';
import ChatPane from '../components/ChatPane';
import { formatRelative } from '../lib/format';

export const chatTitle = (c: Chat) => c.title || (c.username ? `@${c.username}` : String(c.id));

function Sidebar({ selected }: { selected?: number }) {
  const qc = useQueryClient();
  const [filter, setFilter] = useState('');
  const [tab, setTab] = useState<'all' | 'tracked'>('all');
  const [kinds, setKinds] = useState<ChatKind[]>([]);
  const [sort, setSort] = useState<ChatSort>('last_message');
  const chats = useQuery({ queryKey: ['chats', sort], queryFn: () => api.chats(sort) });
  const refresh = useMutation({
    mutationFn: api.refreshChats,
    onSuccess: () => qc.invalidateQueries({ queryKey: ['chats'] }),
  });

  const rows = useMemo(() => {
    const text = filter.trim().toLowerCase();
    return (chats.data ?? []).filter(
      (c) =>
        (tab === 'all' || c.tracked) &&
        (kinds.length === 0 || kinds.includes(c.kind)) &&
        (!text || `${c.title ?? ''} ${c.username ?? ''} ${c.id}`.toLowerCase().includes(text)),
    );
  }, [chats.data, filter, tab, kinds]);

  const refreshError =
    refresh.error instanceof ApiError && refresh.error.status === 503
      ? new Error('目前沒有執行中的 Telegram 收集器（--query-only 或未設定 API 憑證），無法重新整理清單')
      : refresh.error;

  return (
    <aside className="sidebar" aria-label="對話清單">
      <div className="sidebar-head">
        <input
          type="search"
          placeholder="篩選對話…"
          aria-label="篩選對話"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <div className="row">
          <div role="tablist" className="tabs">
            {(['all', 'tracked'] as const).map((t) => (
              <button key={t} role="tab" aria-selected={tab === t} onClick={() => setTab(t)}>
                {t === 'all' ? '全部' : '收集中'}
              </button>
            ))}
          </div>
          <select aria-label="排序" value={sort} onChange={(e) => setSort(e.target.value as ChatSort)}>
            <option value="title">名稱</option>
            <option value="last_message">最後訊息</option>
            <option value="message_count">訊息數</option>
          </select>
        </div>
        <div className="row wrap">
          {(Object.keys(KIND_LABEL) as ChatKind[]).map((k) => (
            <button
              key={k}
              className="pill"
              aria-pressed={kinds.includes(k)}
              onClick={() => setKinds((cur) => (cur.includes(k) ? cur.filter((x) => x !== k) : [...cur, k]))}
            >
              {KIND_LABEL[k]}
            </button>
          ))}
        </div>
        <button disabled={refresh.isPending} onClick={() => refresh.mutate()}>
          {refresh.isPending ? '重新整理中…' : '重新整理清單'}
        </button>
        <ErrorBox error={refreshError} />
      </div>
      <ErrorBox error={chats.error} />
      <ul className="chat-list">
        {chats.isPending && <li className="muted pad">載入中…</li>}
        {rows.map((c) => (
          <li key={c.id}>
            <Link to={`/chats/${c.id}`} className={c.id === selected ? 'chat-row active' : 'chat-row'} aria-current={c.id === selected ? 'page' : undefined}>
              <span aria-label={KIND_LABEL[c.kind]} title={KIND_LABEL[c.kind]}>{KIND_ICON[c.kind]}</span>
              <span className="chat-main">
                <span className="chat-name">{chatTitle(c)}</span>
                <small className="muted">
                  {c.stats ? `${c.stats.message_count.toLocaleString()} 則 · ${formatRelative(c.stats.last_message_at)}` : ' '}
                </small>
              </span>
              {c.stats?.last_error && <span title={c.stats.last_error} aria-label="同步錯誤">⚠️</span>}
              <span className={`dot${c.tracked ? ' on' : ''}`} title={c.tracked ? '收集中' : '未收集'} aria-label={c.tracked ? '收集中' : '未收集'} />
            </Link>
          </li>
        ))}
        {chats.data && rows.length === 0 && <li><Empty>沒有符合的對話</Empty></li>}
      </ul>
    </aside>
  );
}

export default function ChatsPage() {
  const { chatId } = useParams();
  const id = chatId !== undefined && /^-?\d+$/.test(chatId) ? Number(chatId) : undefined;
  return (
    <div className="split">
      <Sidebar selected={id} />
      <section className="pane">
        {id === undefined ? <Empty>{chatId ? '無效的聊天室 ID' : '請從左側選擇一個對話'}</Empty> : <ChatPane key={id} chatId={id} />}
      </section>
    </div>
  );
}
