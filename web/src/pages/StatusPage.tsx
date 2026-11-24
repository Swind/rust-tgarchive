import { useQuery } from '@tanstack/react-query';
import { Activity, ArrowUpRight, Clock3, FileCode2, Server } from 'lucide-react';
import { PageHeader } from '@/components/PageHeader';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader } from '@/components/ui/card';
import { Progress } from '@/components/ui/progress';
import { api } from '../api/client';
import { ErrorBox } from '../components/common';
import { formatDateTime, formatRelative } from '../lib/format';

function Health({ kind }: { kind: 'live' | 'ready' }) {
  const q = useQuery({ queryKey: ['health', kind], queryFn: () => api.health(kind), refetchInterval: 10000 });
  return (
    <Badge variant={q.error ? 'destructive' : q.data ? 'default' : 'secondary'} className="max-w-full whitespace-normal [overflow-wrap:anywhere]">
      {q.error ? q.error.message : q.data?.status ?? '檢查中…'}
    </Badge>
  );
}

export default function StatusPage() {
  const q = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 5000 });
  const s = q.data;
  const r = s?.rate_limit;
  return (
    <div className="mx-auto w-full max-w-6xl space-y-6 p-4 md:p-8">
      <PageHeader eyebrow="封存作業" title="狀態" description="查看收集器、搜尋索引與 API 的運作情形。">
        <Badge variant="outline" className="gap-1.5"><Clock3 aria-hidden="true" />每 5 秒更新狀態</Badge>
      </PageHeader>
      <ErrorBox error={q.error} />
      <div className="grid items-start gap-5 lg:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
        <Card className="min-w-0">
          <CardHeader className="border-b">
            <h2 className="flex items-center gap-2 text-lg font-semibold"><Server aria-hidden="true" className="size-5 text-primary" />執行概況</h2>
            <CardDescription>目前程序的 Telegram 連線、請求與封存指標。</CardDescription>
          </CardHeader>
          <CardContent>
            {q.isPending && <p className="text-sm text-muted-foreground">正在載入系統狀態…</p>}
            {s && (
              <dl className="detail divide-y text-sm [&>div]:grid [&>div]:gap-1.5 [&>div]:py-3.5 [&>div:first-child]:pt-0 [&>div:last-child]:pb-0 [&_dd]:min-w-0 [&_dd]:[overflow-wrap:anywhere] [&_dt]:font-medium [&_dt]:text-muted-foreground sm:[&>div]:grid-cols-[7rem_minmax(0,1fr)]">
                <div>
                  <dt>收集器</dt>
                  <dd className="flex flex-wrap items-center gap-2"><Badge variant="secondary">{s.collector.state}</Badge>{s.collector.detail && <span>{s.collector.detail}</span>}</dd>
                </div>
                <div>
                  <dt>搜尋索引</dt>
                  <dd className="space-y-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <Badge variant={s.search_index.state === 'ready' ? 'default' : 'secondary'}>{s.search_index.state}</Badge>
                      <span className="font-mono text-xs text-muted-foreground">v{s.search_index.version}</span>
                    </div>
                    <p className="tabular-nums">已索引 {s.search_index.indexed.toLocaleString()}／{s.search_index.total.toLocaleString()} 則</p>
                    {s.search_index.total > 0 && <Progress aria-hidden="true" value={Math.min(100, s.search_index.indexed / s.search_index.total * 100)} className="h-1.5" />}
                  </dd>
                </div>
                <div>
                  <dt>請求間隔</dt>
                  <dd className="flex flex-wrap items-center gap-2">
                    {r ? (
                      <>
                        <span className="tabular-nums">目前 {r.interval_ms} ms／基礎 {r.base_interval_ms} ms</span>
                        {r.interval_ms > r.base_interval_ms && <Badge variant="outline">已放慢</Badge>}
                      </>
                    ) : '此程序沒有 Telegram 存取'}
                  </dd>
                </div>
                {r && (
                  <div>
                    <dt>最近 FLOOD_WAIT</dt>
                    <dd>{r.last_flood_wait_secs != null ? `${r.last_flood_wait_secs} 秒` : '無'}{r.last_flood_at && `（${formatDateTime(r.last_flood_at)}，${formatRelative(r.last_flood_at)}）`}</dd>
                  </div>
                )}
                <div>
                  <dt>未歸屬刪除</dt>
                  <dd><strong className="font-semibold tabular-nums">{s.unresolved_deletions.toLocaleString()}</strong><span className="text-muted-foreground">（無法對應到聊天室，僅保留墓碑）</span></dd>
                </div>
                <div>
                  <dt>同步工作</dt>
                  <dd className="font-semibold tabular-nums">{s.sync_jobs.length}</dd>
                </div>
              </dl>
            )}
          </CardContent>
        </Card>
        <div className="grid min-w-0 gap-5">
          <Card>
            <CardHeader>
              <h2 className="flex items-center gap-2 text-lg font-semibold"><Activity aria-hidden="true" className="size-5 text-primary" />健康檢查</h2>
              <CardDescription>每 10 秒確認服務是否運作並可接受請求。</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="divide-y rounded-lg border bg-muted/30 px-3 text-sm">
                <div className="flex items-center justify-between gap-3 py-3"><dt className="font-mono">live</dt><dd className="min-w-0"><Health kind="live" /></dd></div>
                <div className="flex items-center justify-between gap-3 py-3"><dt className="font-mono">ready</dt><dd className="min-w-0"><Health kind="ready" /></dd></div>
              </dl>
            </CardContent>
          </Card>
          <Card>
            <CardHeader>
              <h2 className="flex items-center gap-2 text-lg font-semibold"><FileCode2 aria-hidden="true" className="size-5 text-primary" />API 文件</h2>
              <CardDescription>以 JSON 或 YAML 格式檢視 OpenAPI 規格。</CardDescription>
            </CardHeader>
            <CardContent className="grid gap-2">
              {['/openapi.json', '/openapi.yml'].map((path) => (
                <Button key={path} asChild variant="outline" className="w-full justify-between">
                  <a href={path}><code>{path}</code><ArrowUpRight aria-hidden="true" className="text-muted-foreground" /></a>
                </Button>
              ))}
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
