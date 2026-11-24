import { useEffect, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { useInfiniteQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, Clock, MessagesSquare, MessageSquare } from 'lucide-react';
import { PageHeader } from '@/components/PageHeader';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { Choices, SearchField } from '@/components/DiscoveryControls';
import { initials } from '@/lib/bubble';
import { api, type SenderSort } from '../api/client';
import BotBadge from '../components/BotBadge';
import { Empty, ErrorBox, useDebounced } from '../components/common';
import { KIND_SENDER, senderLabel } from '../components/senderFormat';
import { formatRelative } from '../lib/format';

const SORTS: [SenderSort, string][] = [
  ['messages', '訊息數'],
  ['last_message', '最近發言'],
  ['name', '名稱'],
];

type BotFilter = 'all' | 'human' | 'bot';
const BOT_FILTERS: [BotFilter, string][] = [
  ['all', '全部'],
  ['human', '人'],
  ['bot', 'Bot'],
];

export default function SendersPage() {
  const [params, setParams] = useSearchParams();
  const q = (params.get('q') ?? '').trim();
  const sortParam = params.get('sort');
  const sort: SenderSort = sortParam === 'last_message' || sortParam === 'name' ? sortParam : 'messages';
  const botParam = params.get('bot');
  const bot: BotFilter = botParam === 'human' || botParam === 'bot' ? botParam : 'all';
  const [text, setText] = useState(params.get('q') ?? '');
  const debounced = useDebounced(text.trim());

  const update = (changes: Record<string, string>, replace = false) =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        for (const [k, v] of Object.entries(changes)) {
          if (v) next.set(k, v);
          else next.delete(k);
        }
        return next;
      },
      { replace },
    );
  useEffect(() => {
    if (debounced !== q) update({ q: debounced }, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced]);
  useEffect(() => {
    setText((t) => (t.trim() === q ? t : q));
  }, [q]);

  const list = useInfiniteQuery({
    queryKey: ['senders-list', q, sort, bot],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => api.senderList({ q, sort, is_bot: bot === 'all' ? undefined : bot === 'bot', limit: 100, cursor: pageParam }),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  const items = list.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <div className="mx-auto w-full max-w-6xl space-y-6 p-4 md:p-8">
      <PageHeader eyebrow="封存探索" title="使用者" description="依名稱、帳號或 ID 尋找發言者，查看曾用名稱與跨聊天室的發言紀錄。" />
      <search aria-label="使用者篩選">
        <Card className="gap-4 p-4 sm:p-5">
          <SearchField label="搜尋使用者" placeholder="名稱、@使用者名稱或 ID" value={text} onChange={setText} />
          <div className="flex flex-wrap items-center justify-between gap-3 border-t pt-3">
            <Choices label="類型" choices={BOT_FILTERS} value={bot} onChange={(value) => update({ bot: value === 'all' ? '' : value })} />
            <Choices label="排序" choices={SORTS} value={sort} onChange={(value) => update({ sort: value === 'messages' ? '' : value })} />
          </div>
        </Card>
      </search>
      <ErrorBox error={list.error} />
      {list.isPending && !list.error && <Empty>載入中…</Empty>}
      {list.data && items.length === 0 && <Empty>找不到符合的使用者</Empty>}
      {items.length > 0 && <section aria-labelledby="people-heading" className="@container space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <h2 id="people-heading" className="text-lg font-semibold">使用者清單</h2>
          <Badge variant="secondary" className="tabular-nums">已載入 {items.length.toLocaleString()} 位</Badge>
        </div>
        <ul className="grid gap-3 @xl:grid-cols-2 @4xl:grid-cols-3" aria-label="使用者清單">
          {items.map((s) => (
            <li key={s.id} className="sender-row min-w-0">
              <Card className="relative h-full gap-3 p-4 transition-colors hover:border-primary/40 focus-within:border-primary">
                <div className="flex min-w-0 items-start gap-3">
                  <span aria-hidden="true" className="grid size-10 shrink-0 place-items-center rounded-full bg-primary/10 font-semibold text-primary">{initials(senderLabel(s))}</span>
                  <div className="min-w-0 grow">
                    <Link className="block w-fit max-w-full break-all rounded-sm font-semibold hover:underline focus-visible:outline-2 focus-visible:outline-ring after:absolute after:inset-0" to={`/senders/${s.id}`}>{senderLabel(s)}</Link>
                    <p className="break-all text-xs leading-5 text-muted-foreground">{s.username && <>@{s.username} · </>}ID {s.id}</p>
                    <div className="mt-1.5 flex flex-wrap items-center gap-1">
                      <Badge variant="outline">{KIND_SENDER[s.kind]}</Badge>
                      {s.is_self && <Badge variant="secondary">自己</Badge>}
                      <BotBadge isBot={s.is_bot} />
                    </div>
                  </div>
                  <ChevronRight aria-hidden="true" className="mt-1 size-4 shrink-0 text-muted-foreground" />
                </div>
                {s.matched_history && s.matched_name && <p className="history-hint break-all rounded-md bg-muted px-2.5 py-1.5 text-xs text-muted-foreground">
                  曾用名：{s.matched_name.display_name || '（無名稱）'}{s.matched_name.username && <> @{s.matched_name.username}</>}
                </p>}
                <div className="mt-auto space-y-2 border-t pt-3 text-xs">
                  <div className="flex flex-wrap items-center gap-4 font-medium tabular-nums">
                    <span title="訊息數" className="inline-flex items-center gap-1.5"><MessageSquare aria-hidden="true" className="size-3.5 text-muted-foreground" />{s.message_count.toLocaleString()} 則</span>
                    <span title="聊天室數" className="inline-flex items-center gap-1.5"><MessagesSquare aria-hidden="true" className="size-3.5 text-muted-foreground" />{s.chat_count} 個聊天室</span>
                  </div>
                  <p className="flex items-center gap-1.5 text-muted-foreground"><Clock aria-hidden="true" className="size-3.5" />最近發言：{formatRelative(s.last_message_at)}</p>
                </div>
              </Card>
            </li>
          ))}
        </ul>
      </section>}
      {list.hasNextPage && (
        <Button className="w-full" variant="outline" disabled={list.isFetchingNextPage} onClick={() => void list.fetchNextPage()}>
          <ChevronDown aria-hidden="true" />{list.isFetchingNextPage ? '載入中…' : '載入更多'}
        </Button>
      )}
    </div>
  );
}
