import { Fragment, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { ArrowLeft, ClockArrowLeft, MessagesSquare } from 'lucide-react';
import { PageHeader } from '@/components/PageHeader';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { NativeSelectOption } from '@/components/ui/native-select';
import { ChatFilter, CheckFilter, Choices, DateRange, SEARCH_SORTS, SearchField, SecondaryFilters } from '@/components/DiscoveryControls';
import { api, ApiError } from '../api/client';
import BotBadge from '../components/BotBadge';
import { Empty, ErrorBox, KIND_LABEL, useDebounced } from '../components/common';
import MessageItem from '../components/MessageItem';
import SearchResultLink, { useRestoreSearchScroll } from '../components/SearchResultLink';
import { KIND_SENDER, senderLabel } from '../components/senderFormat';
import { dayKey, formatDate, formatDateTime, formatRelative, localDayToIso } from '../lib/format';
import { nextOlderCursor } from '../lib/messages';
import { chatTitle } from './ChatsPage';

type Sort = 'relevance' | 'time';
const pageClass = 'mx-auto w-full max-w-6xl space-y-5 p-4 md:space-y-6 md:p-8';

function DirectoryLink() {
  return <Button variant="ghost" size="sm" className="-ms-2 w-fit text-muted-foreground" asChild>
    <Link to="/senders"><ArrowLeft aria-hidden="true" />使用者清單</Link>
  </Button>;
}

function BottomSentinel({ onVisible, disabled }: { onVisible: () => void; disabled: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || disabled) return;
    const io = new IntersectionObserver((entries) => entries.some((e) => e.isIntersecting) && onVisible());
    io.observe(el);
    return () => io.disconnect();
  }, [onVisible, disabled]);
  return <div ref={ref} aria-hidden="true" className="h-px" />;
}

export default function SenderPage() {
  const { senderId } = useParams();
  const id = senderId && /^-?\d+$/.test(senderId) ? Number(senderId) : undefined;
  const [params, setParams] = useSearchParams();
  const chatId = params.get('chat') ?? '';
  const from = params.get('from') ?? '';
  const to = params.get('to') ?? '';
  const deleted = params.get('deleted') === '1';
  const q = (params.get('q') ?? '').trim();
  const sort: Sort = params.get('sort') === 'time' ? 'time' : 'relevance';
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

  const sender = useQuery({
    queryKey: ['sender', id, deleted],
    enabled: id !== undefined,
    queryFn: () => api.sender(id as number, deleted),
    retry: (count, error) => !(error instanceof ApiError && error.status === 404) && count < 2,
  });
  const chats = useQuery({ queryKey: ['chats', 'title'], queryFn: () => api.chats('title') });
  const titles = useMemo(() => new Map((chats.data ?? []).map((c) => [c.id, chatTitle(c)])), [chats.data]);

  const common = {
    sender_id: id,
    chat_id: chatId || undefined,
    from: localDayToIso(from, false),
    to: localDayToIso(to, true),
    include_deleted: deleted || undefined,
  };
  const timeline = useInfiniteQuery({
    queryKey: ['sender-messages', id, chatId, from, to, deleted],
    enabled: id !== undefined && q.length === 0,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => api.allMessages({ ...common, before: pageParam, limit: 50 }),
    getNextPageParam: nextOlderCursor,
  });
  const results = useInfiniteQuery({
    queryKey: ['sender-search', id, q, chatId, from, to, deleted, sort],
    enabled: id !== undefined && q.length > 0,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => api.search({ ...common, q, sort, before: pageParam, limit: 30 }),
    getNextPageParam: nextOlderCursor,
  });
  const active = q.length > 0 ? results : timeline;
  const items = active.data?.pages.flatMap((p) => p.items) ?? [];
  useRestoreSearchScroll(!sender.isPending && !active.isPending);

  if (id === undefined) return <div className={pageClass}><DirectoryLink /><Empty>無效的使用者 ID</Empty></div>;
  if (sender.isPending) return <div className={pageClass}><DirectoryLink /><Empty>載入中…</Empty></div>;
  if (sender.error) {
    const notFound = sender.error instanceof ApiError && sender.error.status === 404;
    return (
      <div className={pageClass}>
        <DirectoryLink />
        {notFound ? <Empty>找不到使用者 {id}</Empty> : <ErrorBox error={sender.error} />}
      </div>
    );
  }
  const s = sender.data;
  let lastDay = '';

  return (
    <div className={pageClass}>
      <DirectoryLink />
      <PageHeader eyebrow="使用者檔案" title={senderLabel(s)} description={`${s.username ? `@${s.username} · ` : ''}ID ${s.id}`}>
        <Badge variant="outline">{KIND_SENDER[s.kind]}</Badge>
        {s.is_self && <Badge variant="secondary">自己</Badge>}
        <BotBadge isBot={s.is_bot} />
      </PageHeader>
      <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-xl border bg-border md:grid-cols-4">
        {[
          ['封存訊息', `${s.message_count.toLocaleString()} 則訊息`],
          ['參與對話', `${s.chat_count} 個聊天室`],
          ['最早發言', s.first_message_at ? formatDateTime(s.first_message_at) : '—'],
          ['最近發言', formatRelative(s.last_message_at)],
        ].map(([label, stat]) => <div className="min-w-0 bg-card p-3 sm:p-4" key={label}>
          <dt className="text-xs text-muted-foreground">{label}</dt>
          <dd className="mt-1 text-sm font-semibold tabular-nums sm:text-base">{stat}</dd>
        </div>)}
      </dl>
      <div className="grid items-start gap-5 lg:grid-cols-[17rem_minmax(0,1fr)]">
        <aside className="grid min-w-0 gap-4 sm:grid-cols-2 lg:grid-cols-1" aria-label="使用者資料">
          <section aria-labelledby="sender-chats-heading">
            <Card className="gap-3 p-4">
              <h2 id="sender-chats-heading" className="flex items-center gap-2 font-semibold"><MessagesSquare aria-hidden="true" className="size-4 text-primary" />聊天室分佈</h2>
              <p className="text-xs leading-relaxed text-muted-foreground">點選聊天室以篩選發言紀錄。</p>
              <ul className="sender-chats flex max-h-48 flex-wrap gap-1.5 overflow-auto py-1 lg:max-h-80">
                {s.chats.map((c) => (
                  <li key={c.chat_id} className="max-w-full">
                    <Button type="button" size="sm" variant={chatId === String(c.chat_id) ? 'secondary' : 'outline'}
                      className="h-auto min-h-8 max-w-full gap-2 whitespace-normal aria-pressed:border-primary/40 aria-pressed:text-primary"
                      aria-pressed={chatId === String(c.chat_id)}
                      title={`${KIND_LABEL[c.kind]} · 最近 ${c.last_message_at ? formatDateTime(c.last_message_at) : '—'}`}
                      onClick={() => update({ chat: chatId === String(c.chat_id) ? '' : String(c.chat_id) })}>
                      <span className="min-w-0 break-all">{c.title || c.chat_id}</span><span className="text-xs tabular-nums">{c.message_count}</span>
                    </Button>
                  </li>
                ))}
              </ul>
            </Card>
          </section>
          {s.name_history.length > 1 && <section aria-labelledby="sender-history-heading">
            <Card className="gap-3 p-4">
              <h2 id="sender-history-heading" className="flex items-center gap-2 font-semibold"><ClockArrowLeft aria-hidden="true" className="size-4 text-primary" />曾用名稱</h2>
              <p className="text-xs leading-relaxed text-muted-foreground">時間為封存觀察紀錄，不一定是對方實際改名的時間。</p>
              <ol className="grid max-h-64 gap-3 overflow-y-auto border-s ps-3">
                {s.name_history.map((h, i) => (
                  <li key={`${h.first_seen_at}-${i}`} className="space-y-1 text-sm">
                    <div className="flex flex-wrap items-center gap-1.5"><strong className="break-all">{h.display_name || '（無名稱）'}</strong>{i === 0 && <Badge variant="secondary">目前</Badge>}</div>
                    {h.username && <div className="break-all text-xs text-muted-foreground">@{h.username}</div>}
                    <p className="text-xs leading-relaxed text-muted-foreground">{formatDateTime(h.first_seen_at)}{h.last_seen_at !== h.first_seen_at && <> ～ {formatDateTime(h.last_seen_at)}</>}</p>
                  </li>
                ))}
              </ol>
            </Card>
          </section>}
        </aside>
        <section aria-labelledby="sender-feed-heading" className="min-w-0 space-y-4">
          <h2 id="sender-feed-heading" className="text-lg font-semibold">發言紀錄</h2>
          <search aria-label="使用者發言篩選">
            <Card className="@container gap-4 p-4">
              <SearchField label="在此使用者的發言中搜尋" placeholder="輸入關鍵字，或留白瀏覽時間軸" value={text} onChange={setText} />
              <SecondaryFilters count={[chatId, from, to, deleted, !!q && sort === 'time'].filter(Boolean).length}>
                <div className="grid items-end gap-4 @2xl:grid-cols-[1fr_2fr]">
                  <ChatFilter value={chatId} onChange={(chat) => update({ chat })}>
                    {s.chats.map((c) => <NativeSelectOption key={c.chat_id} value={c.chat_id}>{c.title || c.chat_id}</NativeSelectOption>)}
                  </ChatFilter>
                  <DateRange from={from} to={to} update={update} />
                </div>
                <div className="flex flex-wrap items-center justify-between gap-x-4 border-t pt-2">
                  <CheckFilter label="顯示已刪除" checked={deleted} onChange={(checked) => update({ deleted: checked ? '1' : '' })} />
                  {q && <Choices label="排序" choices={SEARCH_SORTS} value={sort} onChange={(value) => update({ sort: value === 'time' ? 'time' : '' })} />}
                </div>
              </SecondaryFilters>
            </Card>
          </search>

          <ErrorBox error={active.error} />
          {active.isPending && !active.error && <Empty>載入中…</Empty>}
          {active.data && items.length === 0 && <Empty>{q ? '找不到符合的訊息' : '沒有符合的訊息'}</Empty>}
          {q ? (items.length > 0 && <Card className="overflow-hidden py-0">
            <ul className="divide-y" aria-label="搜尋結果">
              {items.map((m) => <SearchResultLink key={`${m.chat_id}-${m.id}`} message={m} query={q} chatTitle={titles.get(m.chat_id)} includeDeleted={deleted} />)}
            </ul>
          </Card>) : (
            <div className="sender-timeline grid gap-2" aria-label="發言時間軸">
              {items.map((m) => {
                const key = dayKey(m.timestamp);
                const sep = key !== lastDay;
                lastDay = key;
                return (
                  <Fragment key={`${m.chat_id}-${m.id}`}>
                    {sep && <div className="flex items-center gap-3 py-2 text-xs font-medium text-muted-foreground before:h-px before:grow before:bg-border after:h-px after:grow after:bg-border">
                      <span>{formatDate(m.timestamp)}</span>
                    </div>}
                    <MessageItem message={m} variant="sender" showChat={titles.get(m.chat_id) ?? String(m.chat_id)} chatTitle={titles.get(m.chat_id)} />
                  </Fragment>
                );
              })}
            </div>
          )}
          <BottomSentinel
            disabled={!active.hasNextPage || active.isFetchingNextPage}
            onVisible={() => void active.fetchNextPage()}
          />
          {active.isFetchingNextPage && <p role="status" className="text-center text-xs text-muted-foreground">載入更多…</p>}
          {!active.hasNextPage && items.length > 0 && <p className="text-center text-xs text-muted-foreground">沒有更多了</p>}
        </section>
      </div>
    </div>
  );
}
