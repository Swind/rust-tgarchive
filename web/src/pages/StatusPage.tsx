import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import { ErrorBox } from '../components/common';
import { formatDateTime, formatRelative } from '../lib/format';

function Health({ kind }: { kind: 'live' | 'ready' }) {
  const q = useQuery({ queryKey: ['health', kind], queryFn: () => api.health(kind), refetchInterval: 10000 });
  const tone = q.data ? 'green' : q.error ? 'red' : 'gray';
  return (
    <span className={`badge ${tone}`}>
      {q.data ? q.data.status : q.error ? ((q.error as Error).message) : '…'}
    </span>
  );
}

export default function StatusPage() {
  const q = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 5000 });
  const s = q.data;
  const r = s?.rate_limit;
  return (
    <div className="page">
      <h2>狀態</h2>
      <ErrorBox error={q.error} />
      {s && (
        <dl className="detail">
          <dt>收集器</dt><dd><strong>{s.collector.state}</strong>{s.collector.detail ? ` — ${s.collector.detail}` : ''}</dd>
          <dt>未歸屬刪除</dt><dd>{s.unresolved_deletions.toLocaleString()}（無法對應到聊天室，僅保留墓碑）</dd>
          <dt>請求間隔</dt>
          <dd>
            {r ? (
              <>
                目前 {r.interval_ms} ms／基礎 {r.base_interval_ms} ms
                {r.interval_ms > r.base_interval_ms && <span className="badge amber">已放慢</span>}
              </>
            ) : (
              '此程序沒有 Telegram 存取'
            )}
          </dd>
          {r && (
            <>
              <dt>最近 FLOOD_WAIT</dt>
              <dd>
                {r.last_flood_wait_secs != null ? `${r.last_flood_wait_secs} 秒` : '無'}
                {r.last_flood_at && `（${formatDateTime(r.last_flood_at)}，${formatRelative(r.last_flood_at)}）`}
              </dd>
            </>
          )}
          <dt>搜尋索引</dt>
          <dd>
            <span className={`badge ${s.search_index.state === 'ready' ? 'green' : 'amber'}`}>{s.search_index.state}</span>{' '}
            v{s.search_index.version}，已索引 {s.search_index.indexed.toLocaleString()}／{s.search_index.total.toLocaleString()} 則
          </dd>
          <dt>同步工作</dt><dd>{s.sync_jobs.length}</dd>
        </dl>
      )}
      <h3>健康檢查</h3>
      <dl className="detail">
        <dt>live</dt><dd><Health kind="live" /></dd>
        <dt>ready</dt><dd><Health kind="ready" /></dd>
      </dl>
      <h3>API 文件</h3>
      <p>
        <a href="/openapi.json">/openapi.json</a> · <a href="/openapi.yml">/openapi.yml</a>
      </p>
    </div>
  );
}
