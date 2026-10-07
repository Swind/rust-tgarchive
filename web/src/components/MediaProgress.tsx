import { useQuery } from '@tanstack/react-query';
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
    <section className="media-progress">
      {!compact && <h3>圖片下載</h3>}
      <div className="muted small">各聊天室的累計下載進度，每 2 秒更新。</div>
      {q.isPending ? <div className="muted">正在載入圖片下載進度…</div> : null}
      {q.error && <ErrorBox error={q.error} />}
      {q.data && rows.length === 0 && <div className="muted">目前沒有圖片下載工作</div>}
      {rows.map((row) => (
        <div className="media-progress-row" key={`${row.chat_id}:${row.variant}`}>
          <strong>{row.title ?? row.chat_id}</strong>
          <span>{labels[row.variant as keyof typeof labels] ?? row.variant}</span>
          <span>排隊 {row.queued.toLocaleString()}</span>
          <span>下載中 {row.running.toLocaleString()}</span>
          <span>完成 {row.succeeded.toLocaleString()}</span>
          <span>失敗 {row.failed.toLocaleString()}{row.retrying ? `（待重試 ${row.retrying.toLocaleString()}）` : ''}</span>
          <span>中斷 {row.interrupted.toLocaleString()}</span>
          <span>無法下載 {row.unavailable.toLocaleString()}</span>
          {row.superseded > 0 && <span>已取代 {row.superseded.toLocaleString()}</span>}
          <span className="muted">共 {row.total.toLocaleString()}</span>
        </div>
      ))}
    </section>
  );
}
