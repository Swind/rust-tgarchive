import { ChevronDown, Clock3, History, RefreshCw, RotateCcw, TriangleAlert } from 'lucide-react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { PageHeader } from '@/components/PageHeader';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader } from '@/components/ui/card';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { api, ApiError, type SyncJob } from '../api/client';
import { Empty, ErrorBox, JobBadge, isActiveJob } from '../components/common';
import MediaProgress from '../components/MediaProgress';
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

function JobDetail({ id, jobs }: { id: string; jobs: SyncJob[] }) {
  const q = useQuery({
    queryKey: ['job', id],
    queryFn: () => api.syncJob(id),
    refetchInterval: (query) => (query.state.data && isActiveJob(query.state.data.state) ? 2000 : false),
  });
  const { retry, error } = useRetry();
  if (q.isPending) return <p className="text-sm text-muted-foreground">正在載入工作明細…</p>;
  if (q.error) return <ErrorBox error={q.error} />;
  const j = q.data;
  return (
    <>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-5 gap-y-2 text-sm [overflow-wrap:anywhere] [&_dt]:text-muted-foreground">
        <dt>工作 ID</dt><dd className="font-mono text-xs">{j.id}</dd>
        <dt>範圍</dt><dd>{j.scope}</dd>
        <dt>狀態</dt><dd><JobBadge state={j.state} /></dd>
        <dt>建立</dt><dd>{formatDateTime(j.created_at)}</dd>
        <dt>開始</dt><dd>{j.started_at ? formatDateTime(j.started_at) : '—'}</dd>
        <dt>結束</dt><dd>{j.completed_at ? formatDateTime(j.completed_at) : '—'}</dd>
        <dt>錯誤</dt><dd>{j.error_summary ?? '無'}</dd>
      </dl>
      <h3 className="mt-5 mb-2 text-sm font-semibold">各聊天室進度</h3>
      {j.chats && j.chats.length > 0 ? (
        <ul className="chat-progress divide-y rounded-lg border bg-card" aria-label="各聊天室進度">
          {j.chats.map((c) => (
            <li key={c.chat_id} className="flex flex-wrap items-center gap-x-3 gap-y-2 px-3 py-3 text-sm">
              <strong className="min-w-0 font-medium [overflow-wrap:anywhere]">{c.title ?? c.chat_id}</strong>
              <JobBadge state={c.state} />
              <span className="text-muted-foreground tabular-nums">已寫入 {c.committed_count.toLocaleString()} 則</span>
              {RETRYABLE.has(c.state) && (
                <Button variant="outline" size="sm" className="ml-auto" disabled={retry.isPending || hasActiveChatSync(jobs, c.chat_id)} onClick={() => retry.mutate(c.chat_id)}>
                  <RotateCcw aria-hidden="true" />重試
                </Button>
              )}
              {c.error_summary && <p className="w-full text-destructive [overflow-wrap:anywhere]">{c.error_summary}</p>}
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-sm text-muted-foreground">沒有逐聊天室的進度</p>
      )}
      <ErrorBox error={error} />
    </>
  );
}

function JobRow({ job, jobs }: { job: SyncJob; jobs: SyncJob[] }) {
  const { retry, error } = useRetry();
  const chatId = /^chat:(-?\d+)$/.exec(job.scope)?.[1];
  return (
    <li className="job px-4 py-3 sm:px-6" data-state={job.state}>
      <Collapsible>
        <div className="flex flex-wrap items-center gap-2">
          <CollapsibleTrigger asChild>
            <Button variant="ghost" className="job-head group h-auto min-w-0 flex-1 justify-start gap-3 px-2 py-2 text-left whitespace-normal">
              <ChevronDown aria-hidden="true" className="text-muted-foreground transition-transform group-data-[state=open]:rotate-180 motion-reduce:transition-none" />
              <span className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="font-semibold">{job.scope === 'all' ? '全部聊天室' : chatId ? '聊天室同步' : '同步工作'}</span>
                <span className="font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]">{job.scope}</span>
              </span>
              <span className="flex flex-col items-end gap-1">
                <JobBadge state={job.state} />
                <time className="text-xs font-normal text-muted-foreground" dateTime={job.created_at} title={formatDateTime(job.created_at)}>{formatRelative(job.created_at)}</time>
              </span>
            </Button>
          </CollapsibleTrigger>
          {chatId && RETRYABLE.has(job.state) && (
            <Button size="sm" variant="outline" disabled={retry.isPending || hasActiveChatSync(jobs, Number(chatId))} onClick={() => retry.mutate(Number(chatId))}>
              <RotateCcw aria-hidden="true" />重試
            </Button>
          )}
        </div>
        {(job.error_summary || job.retry_after_secs != null) && (
          <div className="mx-2 mt-2 space-y-1 rounded-md bg-muted/60 px-3 py-2 text-sm sm:ml-9">
            {job.error_summary && (
              <p className="flex items-start gap-2 [overflow-wrap:anywhere]">
                <TriangleAlert aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-destructive" />
                <span><span className="sr-only">錯誤摘要：</span>{job.error_summary}</span>
              </p>
            )}
            {job.retry_after_secs != null && <p className="flex items-center gap-2 text-xs text-muted-foreground"><Clock3 aria-hidden="true" className="size-4 shrink-0" />約 {job.retry_after_secs} 秒後可重試</p>}
          </div>
        )}
        <ErrorBox error={error} />
        <CollapsibleContent>
          <div className="mt-3 rounded-lg border bg-muted/30 p-4">
            <JobDetail id={job.id} jobs={jobs} />
          </div>
        </CollapsibleContent>
      </Collapsible>
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
    <div className="mx-auto w-full max-w-6xl space-y-6 p-4 md:p-8">
      <PageHeader eyebrow="封存作業" title="同步" description="同步追蹤中的聊天室，查看工作明細與圖片下載進度。">
        <Button variant="outline" onClick={() => void status.refetch()}><RefreshCw aria-hidden="true" />重新整理</Button>
        <Button disabled={syncAll.isPending || syncAllBlocked} aria-describedby={syncAllBlocked ? 'sync-blocked' : undefined} onClick={() => syncAll.mutate()}>
          <RotateCcw aria-hidden="true" />同步全部
        </Button>
      </PageHeader>
      {syncAllBlocked && <p id="sync-blocked" className="flex items-center gap-2 rounded-lg border bg-muted/50 px-4 py-3 text-sm" role="status"><Clock3 aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />{status.isPending ? '正在確認同步狀態…' : '已有同步工作，請稍後再同步全部'}</p>}
      <ErrorBox error={syncConflict(err)} />
      <ErrorBox error={status.error} />
      <Card className="min-w-0 gap-0 pb-0">
        <CardHeader className="border-b pb-5">
          <div className="flex items-center gap-2">
            <History aria-hidden="true" className="size-5 text-primary" />
            <h2 className="text-lg font-semibold">同步工作</h2>
            {status.data && <Badge variant="secondary" className="ml-auto tabular-nums">{jobs.length.toLocaleString()} 筆</Badge>}
          </div>
          <CardDescription>展開工作可查看執行時間、錯誤訊息與各聊天室進度。</CardDescription>
        </CardHeader>
        <CardContent className="px-0">
          {status.isPending && <p className="px-6 py-5 text-sm text-muted-foreground">正在載入同步工作…</p>}
          {status.data && jobs.length === 0 && <Empty>目前沒有同步工作</Empty>}
          {jobs.length > 0 && <ul aria-label="同步工作" className="divide-y">{jobs.map((j) => <JobRow key={j.id} job={j} jobs={jobs} />)}</ul>}
        </CardContent>
      </Card>
      <MediaProgress />
    </div>
  );
}
