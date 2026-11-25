import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { ChevronDown, Info, Search, X } from 'lucide-react';
import { PageHeader } from '@/components/PageHeader';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Label } from '@/components/ui/label';
import { ChatFilter, CheckFilter, Choices, DateRange, SEARCH_SORTS, SearchField, SecondaryFilters } from '@/components/DiscoveryControls';
import { api } from '../api/client';
import { Empty, ErrorBox, useDebounced } from '../components/common';
import { SearchGroupManager } from '@/components/SearchGroupManager';
import SenderPicker from '../components/SenderPicker';
import SearchResultLink, { useRestoreSearchScroll } from '../components/SearchResultLink';
import { localDayToIso } from '../lib/format';
import { nextOlderCursor } from '../lib/messages';
import { parseTopicId, topicLabel } from '../lib/topics';
import { chatTitle } from './ChatsPage';

type Sort = 'relevance' | 'time';

export default function SearchPage() {
  const [params, setParams] = useSearchParams();
  const q = (params.get('q') ?? '').trim();
  const chatId = params.get('chat') ?? '';
  const topicId = chatId ? parseTopicId(params.get('topic_id')) : undefined;
  const groupId = params.get('group_id') ?? '';
  const excluded = [...new Set((params.get('exclude_chat_ids') ?? '').split(',').filter(Boolean))].sort();
  const exclusionParam = excluded.join(',');
  const senderParam = params.get('sender') ?? '';
  const senderId = /^-?\d+$/.test(senderParam) ? Number(senderParam) : undefined;
  const from = params.get('from') ?? '';
  const to = params.get('to') ?? '';
  const deleted = params.get('include_deleted') === '1';
  const hideBots = params.get('hide_bots') === '1';
  const [hideBotsBox, setHideBotsBox] = useState(hideBots);
  useEffect(() => setHideBotsBox(hideBots), [hideBots]);
  const [deletedBox, setDeletedBox] = useState(deleted);
  useEffect(() => setDeletedBox(deleted), [deleted]);
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

  // Typing -> URL (replace, debounced); URL changes from back/forward -> input.
  useEffect(() => {
    if (debounced !== q) update({ q: debounced }, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced]);
  useEffect(() => {
    setText((t) => (t.trim() === q ? t : q));
  }, [q]);
  const enabled = q.length > 0;

  const chats = useQuery({ queryKey: ['chats', 'title'], queryFn: () => api.chats('title') });
  const numericChatId = Number(chatId);
  const topics = useQuery({
    queryKey: ['topics', numericChatId],
    queryFn: () => api.topics(numericChatId),
    enabled: /^-?\d+$/.test(chatId) && Number.isSafeInteger(numericChatId),
  });
  const groups = useQuery({ queryKey: ['search-groups'], queryFn: api.searchGroups, refetchOnWindowFocus: true });
  const group = groups.data?.find((g) => String(g.id) === groupId);
  const members = useMemo(() => (group ? new Set(group.chat_ids) : null), [group]);
  const sources = groupId ? chats.data?.filter((c) => members?.has(c.id)) : chats.data;
  function selectGroup(id: number | undefined) {
    const value = id === undefined ? '' : String(id);
    if (value !== groupId) update({ group_id: value, chat: '', topic_id: '' });
  }
  const titles = useMemo(() => new Map((chats.data ?? []).map((c) => [c.id, chatTitle(c)])), [chats.data]);
  function excludeChat(id: string) {
    if (!id || excluded.includes(id) || excluded.length >= 100) return;
    update({ exclude_chat_ids: [...excluded, id].sort().join(',') });
  }
  function topicSearchTo(chat: number, topic: number) {
    const next = new URLSearchParams(params);
    next.set('chat', String(chat));
    next.set('topic_id', String(topic));
    return `?${next}`;
  }

  const results = useInfiniteQuery({
    queryKey: ['search', q, chatId, topicId, senderId, from, to, deleted, hideBots, sort, exclusionParam, groupId, group?.chat_ids],
    enabled,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      api.search({
        q,
        chat_id: chatId || undefined,
        topic_id: topicId,
        group_id: groupId || undefined,
        exclude_chat_ids: exclusionParam || undefined,
        sender_id: senderId,
        from: localDayToIso(from, false),
        to: localDayToIso(to, true),
        include_deleted: deleted || undefined,
        exclude_bots: hideBots || undefined,
        sort,
        before: pageParam,
        limit: 30,
      }),
    getNextPageParam: nextOlderCursor,
  });
  const items = results.error ? [] : results.data?.pages.flatMap((p) => p.items) ?? [];
  const selectedTopic = topics.data?.find((topic) => topic.id === topicId);
  useRestoreSearchScroll(!results.isPending);
  const status = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 15000 });
  const index = status.data?.search_index;
  const singleChar = [...q].length === 1 && /\p{Script=Han}/u.test(q);

  return (
    <div className="mx-auto w-full max-w-6xl space-y-6 p-4 md:p-8">
      <PageHeader eyebrow="封存探索" title="搜尋" description="搜尋整個封存，或只查看自訂群組內的訊息。">
        {groups.data && chats.data && <SearchGroupManager groups={groups.data} chats={chats.data} selectedId={group?.id} onSelect={selectGroup} />}
      </PageHeader>
      <search aria-label="封存訊息搜尋">
        <Card className="@container gap-4 p-4 sm:p-5">
          <SearchField label="搜尋關鍵字" placeholder="輸入文字、詞彙或英文單字" value={text} onChange={setText} autoFocus />
          <div className="flex min-w-0 flex-wrap items-center gap-2 [&>[data-slot=native-select-wrapper]]:max-w-full">
            <Label htmlFor="search-group">搜尋範圍</Label>
            <NativeSelect id="search-group" value={groupId} onChange={(e) => selectGroup(e.target.value ? Number(e.target.value) : undefined)}>
              <NativeSelectOption value="">全部來源</NativeSelectOption>
              {groupId && !group && <NativeSelectOption value={groupId}>{groups.isPending ? '載入群組中…' : `無法取得群組 #${groupId}`}</NativeSelectOption>}
              {groups.data?.map((g) => <NativeSelectOption key={g.id} value={String(g.id)}>{g.name}（{g.chat_ids.length} 個來源）</NativeSelectOption>)}
            </NativeSelect>
            {(topics.data?.length || topicId !== undefined) ? <div className="flex min-w-0 items-center gap-2 [&>[data-slot=native-select-wrapper]]:min-w-0">
              <Label htmlFor="search-topic" className="shrink-0">話題</Label>
              <NativeSelect id="search-topic" className="truncate text-base pointer-coarse:min-h-12 md:text-sm" value={topicId ?? ''} onChange={(e) => update({ topic_id: e.target.value })}>
                <NativeSelectOption value="">所有話題</NativeSelectOption>
                {topicId !== undefined && !selectedTopic && <NativeSelectOption value={topicId}>
                  {topicLabel(items.find((m) => m.topic_id === topicId)?.topic_title, topicId)}
                </NativeSelectOption>}
                {topics.data?.map((topic) => <NativeSelectOption key={topic.id} value={topic.id}>{topicLabel(topic.title, topic.id)}</NativeSelectOption>)}
              </NativeSelect>
            </div> : null}
            {group && !group.chat_ids.length && <span className="text-sm text-muted-foreground">此群組尚無來源，請先加入頻道或聊天室。</span>}
          </div>
          <ErrorBox error={groups.error ?? chats.error ?? topics.error} />
          {excluded.length > 0 && (
            <div className="flex flex-wrap items-center gap-2" role="group" aria-label="已排除的來源">
              <span className="text-sm text-muted-foreground">已排除</span>
              {excluded.map((id) => {
                const title = titles.get(Number(id)) ?? id;
                return <Button key={id} type="button" size="sm" variant="secondary" aria-label={`取消排除 ${title}`}
                  onClick={() => update({ exclude_chat_ids: excluded.filter((v) => v !== id).join(',') })}>
                  <span className="max-w-48 truncate">{title}</span><X aria-hidden="true" />
                </Button>;
              })}
              {excluded.length > 1 && <Button type="button" size="sm" variant="ghost" onClick={() => update({ exclude_chat_ids: '' })}>清除全部排除</Button>}
            </div>
          )}
          <SecondaryFilters count={[chatId, topicId, senderId != null, from, to, deleted, hideBots, sort === 'time', exclusionParam].filter(Boolean).length}>
            <div className="grid min-w-0 items-end gap-4 @sm:grid-cols-2 @4xl:grid-cols-[1fr_1fr_1.6fr]">
              <ChatFilter value={chatId} onChange={(chat) => update({ chat, topic_id: '', exclude_chat_ids: excluded.filter((id) => id !== chat).join(',') })}>
                {chatId && members && !members.has(Number(chatId)) && <NativeSelectOption value={chatId}>
                  {titles.get(Number(chatId)) ?? chatId}（不在此群組）
                </NativeSelectOption>}
                {sources?.map((c) => <NativeSelectOption key={c.id} value={c.id}>{chatTitle(c)}</NativeSelectOption>)}
              </ChatFilter>
              <SenderPicker value={senderId} onChange={(id) => update({ sender: id === undefined ? '' : String(id) })} />
              <div className="@sm:col-span-2 @4xl:col-span-1"><DateRange from={from} to={to} update={update} /></div>
            </div>
            <div className="grid min-w-0 gap-2 [&>[data-slot=native-select-wrapper]]:max-w-full">
              <Label htmlFor="exclude-source">排除來源（可複選）</Label>
              <NativeSelect id="exclude-source" value="" disabled={excluded.length >= 100} onChange={(e) => excludeChat(e.target.value)}>
                <NativeSelectOption value="">{excluded.length >= 100 ? '最多排除 100 個來源' : '選擇要排除的頻道或聊天室'}</NativeSelectOption>
                {sources?.filter((c) => String(c.id) !== chatId && !excluded.includes(String(c.id))).map((c) =>
                  <NativeSelectOption key={c.id} value={c.id}>{chatTitle(c)}</NativeSelectOption>)}
              </NativeSelect>
            </div>
            <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-1 border-t pt-3">
              <div className="flex flex-wrap items-center gap-x-5">
                <CheckFilter label="包含已刪除" checked={deletedBox} onChange={(checked) => {
                  setDeletedBox(checked);
                  update({ include_deleted: checked ? '1' : '' });
                }} />
                <CheckFilter label="隱藏 bot" checked={hideBotsBox} onChange={(checked) => {
                  setHideBotsBox(checked);
                  update({ hide_bots: checked ? '1' : '' });
                }} />
              </div>
              <Choices label="排序" choices={SEARCH_SORTS} value={sort} onChange={(value) => update({ sort: value === 'time' ? 'time' : '' })} />
            </div>
          </SecondaryFilters>
        </Card>
      </search>
      {index && index.state !== 'ready' && (
        <Alert role="status">
          <Info aria-hidden="true" /><AlertTitle>搜尋索引{index.state === 'rebuilding' ? '重建中' : '尚未建立或已過期'}</AlertTitle>
          <AlertDescription><p>已建立 {index.indexed.toLocaleString()}／{index.total.toLocaleString()} 筆索引。暫時改用較慢的子字串掃描，結果依時間排序。請執行 <code className="break-all">tgarchive search rebuild-index</code>。</p></AlertDescription>
        </Alert>
      )}
      {singleChar && (
        <Alert role="status"><Info aria-hidden="true" /><AlertDescription>單字查詢會逐筆掃描訊息文字（較慢），結果依時間排序。</AlertDescription></Alert>
      )}
      <section aria-labelledby="search-results-heading" className="space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <h2 id="search-results-heading" className="text-lg font-semibold">搜尋結果</h2>
          {results.data && <Badge variant="secondary" className="tabular-nums">已載入 {items.length.toLocaleString()} 則</Badge>}
        </div>
        {!enabled && <Empty><Search className="mx-auto mb-3 size-6" aria-hidden="true" />輸入關鍵字開始搜尋</Empty>}
        <ErrorBox error={results.error} />
        {enabled && results.isPending && !results.error && <Empty>搜尋中…</Empty>}
        {enabled && results.data && items.length === 0 && <Empty>找不到符合的訊息</Empty>}
        {items.length > 0 && <Card className="overflow-hidden py-0">
          <ul className="divide-y" aria-label="搜尋結果">
            {items.map((m) => <SearchResultLink key={`${m.chat_id}-${m.id}`} message={m} chatTitle={titles.get(m.chat_id)} query={q} includeDeleted={deleted} showSender
              topicTo={m.topic_id == null ? undefined : topicSearchTo(m.chat_id, m.topic_id)}
              onExclude={!chatId && excluded.length < 100 ? () => excludeChat(String(m.chat_id)) : undefined} />)}
          </ul>
        </Card>}
        {results.hasNextPage && (
          <Button className="w-full" variant="outline" disabled={results.isFetchingNextPage} onClick={() => void results.fetchNextPage()}>
            <ChevronDown aria-hidden="true" />{results.isFetchingNextPage ? '載入中…' : '載入更多'}
          </Button>
        )}
      </section>
      <details className="rounded-lg border bg-muted/30 px-4 text-sm text-muted-foreground">
        <summary className="cursor-pointer py-3 font-medium text-foreground">搜尋語法與比對方式</summary>
        <p className="max-w-prose pb-4 leading-relaxed">中文以結巴斷詞比對詞彙，也能比對連續字元，例如「北咖啡」可找到「新北咖啡店」。英文支援至少 3 個字元的單字前綴；例如 benchmark 可找到 benchmarks，不區分大小寫與全形／半形。預設依相關性排序，也可切換為時間排序。</p>
      </details>
    </div>
  );
}
