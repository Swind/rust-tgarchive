import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, type SyncJob } from '../api/client';
import { Empty, ErrorBox, JobBadge, isActiveJob } from '../components/common';
import { formatDateTime, formatRelative } from '../lib/format';

function JobDetail({ id }: { id: string }) {
  const q = useQuery({
    queryKey: ['job', id],
    queryFn: () => api.syncJob(id),
    refetchInterval: (query) => (query.state.data && isActiveJob(query.state.data.state) ? 2000 : false),
  });
  if (q.isPending) return <div className="muted">載入中…</div>;
  if (q.error) return <ErrorBox error={q.error} />;
  const j = q.data;
  return (
    <dl className="detail">
      <dt>ID</dt><dd><code>{j.id}</code></dd>
      <dt>範圍</dt><dd>{j.scope}</dd>
      <dt>狀態</dt><dd><JobBadge state={j.state} /></dd>
      <dt>建立</dt><dd>{formatDateTime(j.created_at)}</dd>
      <dt>開始</dt><dd>{j.started_at ? formatDateTime(j.started_at) : '—'}</dd>
      <dt>結束</dt><dd>{j.completed_at ? formatDateTime(j.completed_at) : '—'}</dd>
      <dt>錯誤</dt><dd>{j.has_error ? '有（詳情請見伺服器日誌；已被清理的摘要不經由 API 提供）' : '無'}</dd>
    </dl>
  );
}

function JobRow({ job }: { job: SyncJob }) {
  const [open, setOpen] = useState(false);
  return (
    <li className="job">
      <button className="job-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <JobBadge state={job.state} />
        <span>{job.scope}</span>
        {job.has_error && <span className="error-inline" title="此工作含錯誤">⚠️ 有錯誤</span>}
        {job.state === 'rate_limited' && <small className="muted">遭 Telegram FLOOD_WAIT 限流，會自動等待後重試</small>}
        <span className="spacer" />
        <small className="muted" title={formatDateTime(job.created_at)}>{formatRelative(job.created_at)}</small>
      </button>
      {open && <JobDetail id={job.id} />}
    </li>
  );
}

export default function SyncPage() {
  const qc = useQueryClient();
  const status = useQuery({
    queryKey: ['sync-status'],
    queryFn: api.syncStatus,
    refetchInterval: (q) => (q.state.data?.sync_jobs.some((j) => isActiveJob(j.state)) ? 2000 : false),
  });
  const syncAll = useMutation({
    mutationFn: api.syncAll,
    onSuccess: () => qc.invalidateQueries({ queryKey: ['sync-status'] }),
  });
  const err =
    syncAll.error instanceof ApiError && syncAll.error.status === 503
      ? new Error('目前沒有執行中的 Telegram 收集器，無法同步')
      : syncAll.error;
  const jobs = status.data?.sync_jobs ?? [];
  return (
    <div className="page">
      <h2>同步</h2>
      <div className="row">
        <button className="primary" disabled={syncAll.isPending} onClick={() => syncAll.mutate()}>同步全部</button>
        <button onClick={() => void status.refetch()}>重新整理</button>
      </div>
      <ErrorBox error={err} />
      <ErrorBox error={status.error} />
      {status.data && jobs.length === 0 && <Empty>目前沒有同步工作</Empty>}
      <ul className="jobs">{jobs.map((j) => <JobRow key={j.id} job={j} />)}</ul>
    </div>
  );
}
