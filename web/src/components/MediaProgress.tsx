import { useQuery } from '@tanstack/react-query';
import { Images } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Progress } from '@/components/ui/progress';
import { api } from '../api/client';
import { ErrorBox } from './common';

const labels = { preview: '預覽圖片', archive: '封存版本' } as const;

export default function MediaProgress({ chatId, compact = false }: { chatId?: number; compact?: boolean }) {
  const q = useQuery({
    queryKey: ['media-progress'],
    queryFn: api.mediaProgress,
    refetchInterval: 2000,
  });
  const rows = q.data?.filter((row) => chatId === undefined || row.chat_id === chatId) ?? [];
  return (
    <section aria-label="圖片下載進度" className={`media-progress min-w-0 ${compact ? 'flex-1 basis-40' : 'space-y-4 rounded-xl border bg-card p-4 shadow-sm md:p-6'}`}>
      {!compact && <>
        <h3 className="flex items-center gap-2 font-semibold"><Images aria-hidden="true" className="size-5 text-primary" />圖片下載</h3>
        <p className="text-sm text-muted-foreground">各聊天室的累計下載進度，每 2 秒更新。</p>
      </>}
      {q.isPending && <p className="text-xs text-muted-foreground">正在載入圖片下載進度…</p>}
      <ErrorBox error={q.error} />
      {q.data && rows.length === 0 && <p className="py-1 text-sm text-muted-foreground">目前沒有圖片下載工作</p>}
      {rows.map((row) => {
        const variant = labels[row.variant as keyof typeof labels] ?? row.variant;
        return (
        <div className={`space-y-2 ${compact ? 'py-1.5' : 'border-t pt-4'}`} key={`${row.chat_id}:${row.variant}`}>
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <strong className="wrap-anywhere">{row.title ?? row.chat_id}</strong>
            <Badge variant="secondary">{variant}</Badge>
            <span className="ms-auto text-xs text-muted-foreground">共 {row.total.toLocaleString()}</span>
          </div>
          <Progress className="h-1.5" aria-label={`${row.title ?? row.chat_id} ${variant}完成比例`}
            value={row.total ? 100 * row.succeeded / row.total : 0}
            aria-valuetext={`${row.total.toLocaleString()} 項中已完成 ${row.succeeded.toLocaleString()} 項`} />
          <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground tabular-nums">
            <span>排隊 {row.queued.toLocaleString()}</span>
            <span>下載中 {row.running.toLocaleString()}</span>
            <span>完成 {row.succeeded.toLocaleString()}</span>
            <span className={row.failed ? 'text-destructive' : ''}>失敗 {row.failed.toLocaleString()}{row.retrying ? `（待重試 ${row.retrying.toLocaleString()}）` : ''}</span>
            <span>中斷 {row.interrupted.toLocaleString()}</span>
            <span>無法下載 {row.unavailable.toLocaleString()}</span>
            {row.superseded > 0 && <span>已取代 {row.superseded.toLocaleString()}</span>}
          </div>
        </div>
        );
      })}
    </section>
  );
}
