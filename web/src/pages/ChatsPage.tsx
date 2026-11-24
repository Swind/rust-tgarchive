import { useMemo, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Archive, ArrowLeft, ArrowRight, CircleAlert, FolderArchive, Hash, MessageCircle, Radio, RefreshCw, Search, Users } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { NativeSelect } from '@/components/ui/native-select';
import { Skeleton } from '@/components/ui/skeleton';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { api, ApiError, type Chat, type ChatKind, type ChatSort } from '../api/client';
import { Empty, ErrorBox, KIND_LABEL } from '../components/common';
import ChatPane from '../components/ChatPane';
import { formatRelative } from '../lib/format';
import '../chat.css';

export const chatTitle = (c: Chat) => c.title || (c.username ? `@${c.username}` : String(c.id));

const glyphs = { channel: Radio, supergroup: Hash, group: Users, private: MessageCircle } as const;

function ArchiveOverview({ chats }: { chats?: Chat[] }) {
  return (
    <div className="archive-canvas grid min-h-full flex-1 place-items-center overflow-y-auto p-6 xl:p-12">
      <div className="w-full max-w-xl">
        <div className="mb-5 flex size-14 items-center justify-center rounded-2xl border border-primary/20 bg-card text-primary shadow-sm">
          <FolderArchive aria-hidden="true" className="size-7" />
        </div>
        <p className="mb-1 text-xs font-semibold tracking-widest text-primary">封存總覽</p>
        <h2 className="text-balance text-3xl font-semibold tracking-tight">瀏覽對話紀錄</h2>
        <p className="mt-2 text-sm leading-relaxed text-muted-foreground">請從左側選擇一個對話，或搜尋所有已封存的訊息。</p>
        {chats && (
          <Card className="my-6 py-0">
            <CardContent className="grid grid-cols-3 divide-x px-0 py-5">
              {([
                ['對話總數', chats.length],
                ['收集中', chats.filter((c) => c.tracked).length],
                ['封存訊息', chats.reduce((n, c) => n + (c.stats?.message_count ?? 0), 0)],
              ] as const).map(([label, value]) => (
                <dl key={label} className="px-4 lg:px-6">
                  <dt className="text-xs text-muted-foreground">{label}</dt>
                  <dd className="mt-1 text-xl font-semibold tabular-nums lg:text-2xl">{value.toLocaleString()}</dd>
                </dl>
              ))}
            </CardContent>
          </Card>
        )}
        <Button variant="outline" className="mt-2" asChild>
          <Link to="/search"><Search aria-hidden="true" />搜尋封存訊息<ArrowRight aria-hidden="true" /></Link>
        </Button>
      </div>
    </div>
  );
}

export default function ChatsPage() {
  const { chatId } = useParams();
  const id = chatId !== undefined && /^-?\d+$/.test(chatId) ? Number(chatId) : undefined;
  const qc = useQueryClient();
  const [filter, setFilter] = useState('');
  const [tab, setTab] = useState('all');
  const [kinds, setKinds] = useState<ChatKind[]>([]);
  const [sort, setSort] = useState<ChatSort>('last_message');
  const chats = useQuery({ queryKey: ['chats', sort], queryFn: () => api.chats(sort) });
  const refresh = useMutation({
    mutationFn: api.refreshChats,
    onSuccess: () => qc.invalidateQueries({ queryKey: ['chats'] }),
  });

  const rows = useMemo(() => {
    const text = filter.trim().toLowerCase();
    return (chats.data ?? []).filter(
      (c) =>
        (tab === 'all' || c.tracked) &&
        (kinds.length === 0 || kinds.includes(c.kind)) &&
        (!text || `${c.title ?? ''} ${c.username ?? ''} ${c.id}`.toLowerCase().includes(text)),
    );
  }, [chats.data, filter, tab, kinds]);

  const refreshError =
    refresh.error instanceof ApiError && refresh.error.status === 503
      ? new Error('目前沒有執行中的 Telegram 收集器（--query-only 或未設定 API 憑證），無法重新整理清單')
      : refresh.error;

  return (
    <div className="chat-workspace flex h-full min-h-0 min-w-0">
      <Tabs role="complementary" aria-label="對話清單" value={tab} onValueChange={setTab}
        className={`${chatId === undefined ? 'flex' : 'hidden'} min-h-0 w-full shrink-0 flex-col gap-0 bg-card lg:flex lg:w-80 lg:border-r xl:w-[22rem]`}>
        <div className="max-h-[75%] shrink-0 space-y-4 overflow-y-auto overscroll-contain border-b px-4 py-4 xl:px-5">
          <div className="flex items-center gap-3">
            <div className="flex size-10 items-center justify-center rounded-xl bg-primary text-primary-foreground">
              <Archive className="size-5" aria-hidden="true" />
            </div>
            <div className="min-w-0 flex-1">
              <h1 className="text-xl font-semibold tracking-tight">對話封存</h1>
              <p className="text-xs text-muted-foreground">瀏覽與管理已封存的對話</p>
            </div>
            <Button size="icon" variant="ghost" aria-label="重新整理清單" title="重新整理清單" disabled={refresh.isPending} onClick={() => refresh.mutate()}>
              <RefreshCw aria-hidden="true" className={refresh.isPending ? 'motion-safe:animate-spin' : ''} />
            </Button>
          </div>
          <div className="relative">
            <Search className="pointer-events-none absolute start-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <Input className="bg-background ps-9" type="search" placeholder="篩選對話…" aria-label="篩選對話" value={filter} onChange={(e) => setFilter(e.target.value)} />
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <TabsList aria-label="收集狀態">
              <TabsTrigger value="all">全部</TabsTrigger>
              <TabsTrigger value="tracked">收集中</TabsTrigger>
            </TabsList>
            <NativeSelect size="sm" className="text-xs" aria-label="排序" value={sort} onChange={(e) => setSort(e.target.value as ChatSort)}>
              <option value="title">名稱</option>
              <option value="last_message">最後訊息</option>
              <option value="message_count">訊息數</option>
            </NativeSelect>
          </div>
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="對話類型">
            {(Object.keys(KIND_LABEL) as ChatKind[]).map((k) => (
              <Button
                key={k}
                size="sm"
                variant={kinds.includes(k) ? 'default' : 'outline'}
                className="h-7 rounded-full px-2.5 text-xs"
                aria-pressed={kinds.includes(k)}
                onClick={() => setKinds((cur) => (cur.includes(k) ? cur.filter((x) => x !== k) : [...cur, k]))}
              >
                {KIND_LABEL[k]}
              </Button>
            ))}
          </div>
          <ErrorBox error={refreshError} />
          <ErrorBox error={chats.error} />
        </div>
        <TabsContent value={tab} className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pb-3">
          <div className="flex items-center gap-2 px-2.5 pb-2 pt-4 text-xs font-semibold text-muted-foreground">
            對話清單 {chats.data && <Badge variant="secondary" className="px-1.5 tabular-nums">{rows.length.toLocaleString()}</Badge>}
          </div>
          <ul className="chat-list space-y-0.5">
            {chats.isPending && <li aria-label="載入中" className="space-y-2 p-2">{[0, 1, 2].map((row) => <Skeleton key={row} className="h-14 rounded-lg" />)}</li>}
            {rows.map((c) => {
              const Glyph = glyphs[c.kind];
              return (
                <li key={c.id}>
                  <Link to={`/chats/${c.id}`} aria-current={c.id === id ? 'page' : undefined}
                    className="flex min-w-0 items-center gap-3 rounded-lg border border-transparent px-2.5 py-3 hover:bg-muted/70 focus-visible:outline-2 focus-visible:outline-ring aria-[current=page]:border-primary/20 aria-[current=page]:bg-primary/8">
                    <span className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted text-foreground" aria-hidden="true">
                      <Glyph className="size-4.5" aria-hidden="true" />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-semibold" title={chatTitle(c)}>{chatTitle(c)}</span>
                      <span className="mt-1 flex flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground">
                        <span>{KIND_LABEL[c.kind]}</span>
                        {c.stats && <><span aria-hidden="true">·</span><span>{c.stats.message_count.toLocaleString()} 則</span></>}
                      </span>
                    </span>
                    <span className="flex shrink-0 flex-col items-end gap-1.5 text-xs text-muted-foreground">
                      {c.stats?.last_message_at && <time dateTime={c.stats.last_message_at}>{formatRelative(c.stats.last_message_at)}</time>}
                      <span className="flex items-center gap-1.5">
                        {c.stats?.last_error && <span aria-label="同步錯誤" title={c.stats.last_error}><CircleAlert aria-hidden="true" className="size-3.5 text-destructive" /></span>}
                        <span role="img" aria-label={c.tracked ? '收集中' : '未收集'} title={c.tracked ? '收集中' : '未收集'}
                          className={`size-2 rounded-full ${c.tracked ? 'bg-primary' : 'border border-muted-foreground'}`} />
                      </span>
                    </span>
                  </Link>
                </li>
              );
            })}
            {chats.data && rows.length === 0 && <li><Empty>沒有符合的對話</Empty></li>}
          </ul>
        </TabsContent>
      </Tabs>
      <section className={`${chatId === undefined ? 'hidden' : 'flex'} min-h-0 min-w-0 flex-1 flex-col lg:flex`} aria-label="對話內容">
        {chatId !== undefined && (
          <div className="shrink-0 border-b bg-card px-2 lg:hidden">
            <Button asChild size="sm" variant="ghost"><Link to="/chats"><ArrowLeft aria-hidden="true" />返回對話清單</Link></Button>
          </div>
        )}
        {id !== undefined ? <ChatPane key={id} chatId={id} /> : chatId ? <Empty>無效的聊天室 ID</Empty> : <ArchiveOverview chats={chats.data} />}
      </section>
    </div>
  );
}
