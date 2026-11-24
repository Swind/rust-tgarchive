import { useEffect, useState } from 'react';
import { ApiError } from '../api/client';
import { CircleAlert } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Empty as EmptyState } from '@/components/ui/empty';

export function ErrorBox({ error }: { error: unknown }) {
  if (!error) return null;
  const e = error instanceof ApiError ? error : null;
  return (
    <Alert variant="destructive" className="my-2">
      <CircleAlert aria-hidden="true" />
      <AlertTitle>{e?.status === 0 ? '網路錯誤' : '發生錯誤'}</AlertTitle>
      <AlertDescription className="[overflow-wrap:anywhere]">
        {error instanceof Error ? error.message : String(error)}
        {e && e.status > 0 && (
          <small>{e.code}，HTTP {e.status}{e.requestId ? `，request_id: ${e.requestId}` : ''}</small>
        )}
      </AlertDescription>
    </Alert>
  );
}

export const Empty = ({ children }: { children: React.ReactNode }) => (
  <EmptyState className="text-muted-foreground">{children}</EmptyState>
);

export function useDebounced<T>(value: T, ms = 400): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

export const KIND_LABEL = { channel: '頻道', supergroup: '超級群組', group: '群組', private: '私訊' } as const;

const JOB_LABEL: Record<string, string> = {
  queued: '排隊中',
  running: '執行中',
  succeeded: '成功',
  failed: '失敗',
  rate_limited: '被限流（等待後重試）',
  interrupted: '已中斷',
};
const JOB_TONE: Record<string, string> = {
  queued: 'text-primary',
  running: 'bg-accent text-accent-foreground',
  succeeded: 'text-emerald-700 dark:text-emerald-300',
  interrupted: 'text-amber-800 dark:text-amber-300',
  failed: 'text-destructive',
  rate_limited: 'text-amber-800 dark:text-amber-300',
};

export const JobBadge = ({ state }: { state: string }) => (
  <Badge variant="outline" className={JOB_TONE[state]}>{JOB_LABEL[state] ?? state}</Badge>
);

export const isActiveJob = (state: string) => state === 'queued' || state === 'running';
