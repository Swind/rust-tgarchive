import { useQuery } from '@tanstack/react-query';
import { api, ApiError } from '../api/client';
import { isActiveJob } from '../components/common';

export function useSyncStatus() {
  return useQuery({
    queryKey: ['sync-status'],
    queryFn: api.syncStatus,
    refetchInterval: (q) => (q.state.data?.sync_jobs.some((j) => isActiveJob(j.state)) ? 2000 : 5000),
  });
}

export function syncConflict(error: unknown): unknown {
  if (!(error instanceof ApiError) || error.status !== 409 || error.code !== 'conflict') return error;
  return new ApiError('已有同步工作正在進行，請稍後查看「同步」頁面。', error.status, error.code, error.requestId);
}

export const hasActiveChatSync = (jobs: { scope: string; state: string }[], chatId: number) =>
  jobs.some((j) => isActiveJob(j.state) && (j.scope === 'all' || j.scope === `chat:${chatId}`));

export const hasActiveSync = (jobs: { state: string }[]) => jobs.some((j) => isActiveJob(j.state));
