import { useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, type Chat } from '../api/client';
import { ErrorBox, KIND_LABEL, JobBadge } from './common';
import { ContextTimeline, ListTimeline } from './Timeline';
import { formatDateTime, formatRelative, localDayToIso } from '../lib/format';

function Actions({ chat }: { chat: Chat }) {
  const qc = useQueryClient();
  const [backfill, setBackfill] = useState(false);
  const [note, setNote] = useState<React.ReactNode>(null);
  const done = () => {
    void qc.invalidateQueries({ queryKey: ['chats'] });
    void qc.invalidateQueries({ queryKey: ['chat', chat.id] });
  };
  const track = useMutation({
    mutationFn: () => api.track(chat.id, backfill),
    onSuccess: (r) => {
      done();
      setNote(
        r.backfill_job_id ? (
          <>補歷史工作 <code>{r.backfill_job_id}</code>（{r.backfill === 'already_running' ? '沿用執行中的工作' : '已排入佇列'}）</>
        ) : (
          '已開始收集'
        ),
      );
    },
  });
  const untrack = useMutation({
    mutationFn: () => api.untrack(chat.id),
    onSuccess: () => {
      done();
      setNote('已停止收集（已存訊息保留）');
    },
  });
  const sync = useMutation({
    mutationFn: () => api.syncChat(chat.id),
    onSuccess: (job) => {
      done();
      setNote(<>已建立同步工作 <code>{job.id}</code> <JobBadge state={job.state} /></>);
    },
  });
  const syncError =
    sync.error instanceof ApiError && sync.error.status === 409 && sync.error.code === 'chat_not_tracked'
      ? new Error('此聊天室尚未開始收集，請先按「開始收集」')
      : sync.error;
  return (
    <div className="actions">
      {chat.tracked ? (
        <button
          disabled={untrack.isPending}
          onClick={() => window.confirm('確定停止收集此聊天室？已存訊息會保留。') && untrack.mutate()}
        >
          停止收集
        </button>
      ) : (
        <>
          <button className="primary" disabled={track.isPending} onClick={() => track.mutate()}>
            開始收集
          </button>
          <label>
            <input type="checkbox" checked={backfill} onChange={(e) => setBackfill(e.target.checked)} /> 同時補歷史
          </label>
        </>
      )}
      <button disabled={sync.isPending} onClick={() => sync.mutate()}>
        同步
      </button>
      {note && <span className="note" role="status">{note}</span>}
      <ErrorBox error={track.error ?? untrack.error ?? syncError} />
    </div>
  );
}

export default function ChatPane({ chatId }: { chatId: number }) {
  const [params, setParams] = useSearchParams();
  const chat = useQuery({ queryKey: ['chat', chatId], queryFn: () => api.chat(chatId) });
  const senders = useQuery({ queryKey: ['senders', chatId], queryFn: () => api.senders(chatId) });

  const messageParam = params.get('message');
  const messageId = messageParam && /^\d+$/.test(messageParam) ? Number(messageParam) : undefined;
  const sender = params.get('sender');
  const date = params.get('date') ?? '';
  const includeDeleted = params.get('deleted') === '1';

  const update = (key: string, value: string | null) => {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    if (key !== 'message') next.delete('message');
    setParams(next);
  };

  if (chat.isPending) return <div className="empty">載入中…</div>;
  if (chat.error) return <ErrorBox error={chat.error} />;
  const c = chat.data;
  const s = c.stats;
  const filters = {
    senderId: sender && /^-?\d+$/.test(sender) ? Number(sender) : undefined,
    toIso: localDayToIso(date, true),
    includeDeleted,
  };

  return (
    <div className="chat-pane">
      <header className="chat-head">
        <h2>
          {c.title || c.id} <span className="badge gray">{KIND_LABEL[c.kind]}</span>{' '}
          <span className={`badge ${c.tracked ? 'green' : 'gray'}`}>{c.tracked ? '收集中' : '未收集'}</span>
        </h2>
        <div className="muted small">
          {c.username && <a href={`https://t.me/${c.username}`} target="_blank" rel="noreferrer noopener">@{c.username}</a>} ID {c.id}
          {s && (
            <>
              {' · '}{s.message_count.toLocaleString()} 則 · 已刪除 {s.deleted_count} · {s.history_complete ? '歷史完整' : '歷史未完整'}
              {s.first_message_at && <> · 最早 {formatDateTime(s.first_message_at)}</>}
              {s.last_message_at && <> · 最新 {formatRelative(s.last_message_at)}</>}
              {s.last_sync_completed_at && <> · 上次同步 {formatRelative(s.last_sync_completed_at)}</>}
            </>
          )}
        </div>
        {s?.last_error && <div className="error">⚠️ 上次同步失敗：{s.last_error}</div>}
        <Actions chat={c} />
        <div className="filters">
          <select aria-label="寄件人" value={sender ?? ''} onChange={(e) => update('sender', e.target.value || null)}>
            <option value="">所有寄件人</option>
            {senders.data?.map((x) => (
              <option key={x.id} value={x.id}>
                {x.display_name || (x.username ? `@${x.username}` : x.id)}（{x.message_count}）
              </option>
            ))}
          </select>
          <label>
            跳至日期 <input type="date" value={date} onChange={(e) => update('date', e.target.value || null)} />
          </label>
          <label>
            <input type="checkbox" checked={includeDeleted} onChange={(e) => update('deleted', e.target.checked ? '1' : null)} /> 顯示已刪除
          </label>
          {messageId !== undefined && (
            <button onClick={() => update('message', null)}>離開脈絡檢視 ✕</button>
          )}
        </div>
      </header>
      {messageId !== undefined ? (
        <ContextTimeline key={`${messageId}-${includeDeleted}`} chatId={chatId} messageId={messageId} includeDeleted={includeDeleted} chatTitle={c.title} />
      ) : (
        <ListTimeline key={`${sender}-${date}-${includeDeleted}`} chatId={chatId} filters={filters} chatTitle={c.title} />
      )}
    </div>
  );
}
