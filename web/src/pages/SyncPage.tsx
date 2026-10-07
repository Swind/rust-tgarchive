import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, type SyncJob } from '../api/client';
import { Empty, ErrorBox, JobBadge, isActiveJob } from '../components/common';
import { formatDateTime, formatRelative } from '../lib/format';
import { hasActiveChatSync, hasActiveSync, syncConflict, useSyncStatus } from '../lib/syncStatus';

const RETRYABLE = new Set(['failed', 'rate_limited', 'interrupted']);

function useRetry() {
  const qc = useQueryClient();
  const retry = useMutation({
    mutationFn: (chatId: number) => api.syncChat(chatId),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['sync-status'] }),
    onError: (error) => {
      if (syncConflict(error) !== error) void qc.invalidateQueries({ queryKey: ['sync-status'] });
    },
  });
  const error =
    retry.error instanceof ApiError && retry.error.status === 503
      ? new Error('目前沒有執行中的 Telegram 收集器，無法重試')
      : syncConflict(retry.error);
  return { retry, error };
}

function RetryError({ error }: { error: unknown }) {
  return <ErrorBox error={error} />;
}

function JobDetail({ id, jobs }: { id: string; jobs: SyncJob[] }) {
  const q = useQuery({
    queryKey: ['job', id],
    queryFn: () => api.syncJob(id),
    refetchInterval: (query) => (query.state.data && isActiveJob(query.state.data.state) ? 2000 : false),
  });
  const { retry, error } = useRetry();
  if (q.isPending) return <div className="muted">載入中…</div>;
  if (q.error) return <ErrorBox error={q.error} />;
  const j = q.data;
  return (
    <>
      <dl className="detail">
        <dt>ID</dt><dd><code>{j.id}</code></dd>
        <dt>範圍</dt><dd>{j.scope}</dd>
        <dt>狀態</dt><dd><JobBadge state={j.state} /></dd>
        <dt>建立</dt><dd>{formatDateTime(j.created_at)}</dd>
        <dt>開始</dt><dd>{j.started_at ? formatDateTime(j.started_at) : '—'}</dd>
        <dt>結束</dt><dd>{j.completed_at ? formatDateTime(j.completed_at) : '—'}</dd>
        <dt>錯誤</dt><dd>{j.error_summary ?? '無'}</dd>
      </dl>
      <h4 className="pad">各聊天室進度</h4>
      {j.chats && j.chats.length > 0 ? (
        <ul className="chat-progress" aria-label="各聊天室進度">
          {j.chats.map((c) => (
            <li key={c.chat_id}>
              <strong>{c.title ?? c.chat_id}</strong>
              <JobBadge state={c.state} />
              <span className="muted">已寫入 {c.committed_count.toLocaleString()} 則</span>
              {c.error_summary && <span className="error-inline">{c.error_summary}</span>}
              {RETRYABLE.has(c.state) && (
                <button disabled={retry.isPending || hasActiveChatSync(jobs, c.chat_id)} onClick={() => retry.mutate(c.chat_id)}>重試</button>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <div className="muted pad">沒有逐聊天室的進度</div>
      )}
      <RetryError error={error} />
    </>
  );
}

function JobRow({ job, jobs }: { job: SyncJob; jobs: SyncJob[] }) {
  const [open, setOpen] = useState(false);
  const { retry, error } = useRetry();
  const chatId = /^chat:(-?\d+)$/.exec(job.scope)?.[1];
  return (
    <li className="job" data-state={job.state}>
      <div className="job-actions">
        <button className="job-head" aria-expanded={open} onClick={() => setOpen(!open)}>
          <JobBadge state={job.state} />
          <span>{job.scope}</span>
          <span className="spacer" />
          <small className="muted" title={formatDateTime(job.created_at)}>{formatRelative(job.created_at)}</small>
        </button>
        {chatId && RETRYABLE.has(job.state) && (
          <button disabled={retry.isPending || hasActiveChatSync(jobs, Number(chatId))} onClick={() => retry.mutate(Number(chatId))}>重試</button>
        )}
      </div>
      {(job.error_summary || job.retry_after_secs != null) && (
        <div className="job-summary">
          {job.error_summary && <span className="error-inline">⚠️ {job.error_summary}</span>}
          {job.retry_after_secs != null && <small className="muted">約 {job.retry_after_secs} 秒後可重試</small>}
        </div>
      )}
      <RetryError error={error} />
      {open && <JobDetail id={job.id} jobs={jobs} />}
    </li>
  );
}

export default function SyncPage() {
  const qc = useQueryClient();
  const status = useSyncStatus();
  const syncAll = useMutation({
    mutationFn: api.syncAll,
    onSuccess: () => qc.invalidateQueries({ queryKey: ['sync-status'] }),
    onError: (error) => {
      if (syncConflict(error) !== error) void qc.invalidateQueries({ queryKey: ['sync-status'] });
    },
  });
  const err =
    syncAll.error instanceof ApiError && syncAll.error.status === 503
      ? new Error('目前沒有執行中的 Telegram 收集器，無法同步')
      : syncAll.error;
  const jobs = status.data?.sync_jobs ?? [];
  const syncAllBlocked = status.isPending || hasActiveSync(jobs);
  return (
    <div className="page">
      <h2>同步</h2>
      <div className="row">
        <button className="primary" disabled={syncAll.isPending || syncAllBlocked} onClick={() => syncAll.mutate()}>同步全部</button>
        {syncAllBlocked && <span className="note" role="status">{status.isPending ? '正在確認同步狀態…' : '已有同步工作，請稍後再同步全部'}</span>}
        <button onClick={() => void status.refetch()}>重新整理</button>
      </div>
      <ErrorBox error={syncConflict(err)} />
      <ErrorBox error={status.error} />
      {status.data && jobs.length === 0 && <Empty>目前沒有同步工作</Empty>}
      <ul className="jobs">{jobs.map((j) => <JobRow key={j.id} job={j} jobs={jobs} />)}</ul>
    </div>
  );
}
