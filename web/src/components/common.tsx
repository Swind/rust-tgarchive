import { useEffect, useState } from 'react';
import { ApiError } from '../api/client';

export function ErrorBox({ error }: { error: unknown }) {
  if (!error) return null;
  const e = error instanceof ApiError ? error : null;
  return (
    <div className="error" role="alert">
      <strong>{e?.status === 0 ? '網路錯誤' : '發生錯誤'}</strong>：{error instanceof Error ? error.message : String(error)}
      {e && e.status > 0 && (
        <small>
          {' '}
          （{e.code}，HTTP {e.status}
          {e.requestId ? `，request_id: ${e.requestId}` : ''}）
        </small>
      )}
    </div>
  );
}

export const Empty = ({ children }: { children: React.ReactNode }) => <div className="empty">{children}</div>;

export function useDebounced<T>(value: T, ms = 400): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

export const KIND_LABEL = { channel: '頻道', supergroup: '超級群組', group: '群組', private: '私訊' } as const;
export const KIND_ICON = { channel: '📢', supergroup: '👥', group: '👪', private: '👤' } as const;

const JOB_LABEL: Record<string, string> = {
  queued: '排隊中',
  running: '執行中',
  succeeded: '成功',
  failed: '失敗',
  rate_limited: '被限流（等待後重試）',
  interrupted: '已中斷',
};
const JOB_TONE: Record<string, string> = {
  queued: 'blue',
  running: 'blue',
  succeeded: 'green',
  interrupted: 'amber',
  failed: 'red',
  rate_limited: 'amber',
};

export const JobBadge = ({ state }: { state: string }) => (
  <span className={`badge ${JOB_TONE[state] ?? 'gray'}`}>{JOB_LABEL[state] ?? state}</span>
);

export const isActiveJob = (state: string) => state === 'queued' || state === 'running';
