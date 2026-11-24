import { useId, useState } from 'react';
import { Link, useLocation, useSearchParams } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Archive, ChevronDown, CircleAlert, Download, Image as ImageIcon, Info, ListFilter, MessageSquare, Pause, RefreshCw } from 'lucide-react';
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { NativeSelect } from '@/components/ui/native-select';
import { api, ApiError, type Chat } from '../api/client';
import { ErrorBox, KIND_LABEL, JobBadge } from './common';
import { hasActiveChatSync, syncConflict, useSyncStatus } from '../lib/syncStatus';
import { ContextTimeline, ListTimeline } from './Timeline';
import { formatDateTime, formatRelative, localDayToIso } from '../lib/format';
import MediaProgress from './MediaProgress';
import { searchReturnFrom } from './SearchResultLink';

function Actions({ chat }: { chat: Chat }) {
  const qc = useQueryClient();
  const [backfill, setBackfill] = useState(false);
  const [note, setNote] = useState<React.ReactNode>(null);
  const status = useSyncStatus();
  const activeSync = status.data ? hasActiveChatSync(status.data.sync_jobs, chat.id) : false;
  const syncBlocked = status.isPending || activeSync;
  const done = () => {
    void qc.invalidateQueries({ queryKey: ['chats'] });
    void qc.invalidateQueries({ queryKey: ['chat', chat.id] });
  };
  const track = useMutation({
    mutationFn: () => api.track(chat.id, backfill),
    onSuccess: async (r) => {
      done();
      if (r.backfill_job_id) await qc.invalidateQueries({ queryKey: ['sync-status'] });
      setNote(
        r.backfill_job_id ? (
          <>補歷史工作 <code>{r.backfill_job_id}</code>（{r.backfill === 'already_running' ? '沿用執行中的工作' : '已排入佇列'}）</>
        ) : (
          '已開始收集'
        ),
      );
    },
  });
  const untrack = useMutation({
    mutationFn: () => api.untrack(chat.id),
    onSuccess: () => {
      done();
      setNote('已停止收集（已存訊息保留）');
    },
  });
  const sync = useMutation({
    mutationFn: () => api.syncChat(chat.id),
    onSuccess: async (job) => {
      done();
      await qc.invalidateQueries({ queryKey: ['sync-status'] });
      setNote(<>已建立同步工作 <code>{job.id}</code> <JobBadge state={job.state} /></>);
    },
    onError: (error) => {
      if (syncConflict(error) !== error) void qc.invalidateQueries({ queryKey: ['sync-status'] });
    },
  });
  const backfillId = useId();
  const syncError =
    sync.error instanceof ApiError && sync.error.status === 409 && sync.error.code === 'chat_not_tracked'
      ? new Error('此聊天室尚未開始收集，請先按「開始收集」')
      : syncConflict(sync.error);
  return (
    <div className="flex flex-wrap items-center gap-2">
      {chat.tracked ? (
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button size="sm" variant="outline" disabled={untrack.isPending}><Pause aria-hidden="true" />停止收集</Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>停止收集此對話？</AlertDialogTitle>
              <AlertDialogDescription>已封存的訊息會保留。之後可以重新開始收集。</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>取消</AlertDialogCancel>
              <AlertDialogAction onClick={() => untrack.mutate()}>確認停止收集</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      ) : (
        <>
          <Button size="sm" disabled={track.isPending} onClick={() => track.mutate()}>
            <Archive aria-hidden="true" />開始收集
          </Button>
          <Label htmlFor={backfillId} className="min-h-8 cursor-pointer text-xs">
            <Checkbox id={backfillId} checked={backfill} onCheckedChange={(v) => setBackfill(v === true)} /> 同時補歷史
          </Label>
        </>
      )}
      <Button size="sm" variant="outline" disabled={sync.isPending || syncBlocked} onClick={() => sync.mutate()}>
        <RefreshCw aria-hidden="true" />同步
      </Button>
      {syncBlocked && <span className="basis-full text-xs text-muted-foreground" role="status">{status.isPending ? '正在確認同步狀態…' : <>此聊天室已有同步工作，請稍後查看<Link className="text-primary underline underline-offset-4" to="/sync">同步頁面</Link></>}</span>}
      {note && <span className="basis-full wrap-anywhere text-xs text-muted-foreground" role="status">{note}</span>}
      <ErrorBox error={track.error ?? untrack.error ?? syncError ?? status.error} />
    </div>
  );
}

function MediaTools({ chatId }: { chatId: number }) {
  const toolsId = useId();
  const [expanded, setExpanded] = useState(false);
  const [mediaPolicyDraft, setMediaPolicyDraft] = useState<boolean | null>(null);
  const queryClient = useQueryClient();
  const policy = useQuery({ queryKey: ['media-policy', chatId], queryFn: () => api.mediaPolicy(chatId) });
  const savePolicy = useMutation({
    mutationFn: (autoArchive: boolean) => api.setMediaPolicy(chatId, autoArchive),
    onSuccess: (value) => {
      queryClient.setQueryData(['media-policy', chatId], value);
      setMediaPolicyDraft(null);
    },
    onError: () => setMediaPolicyDraft(null),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: ['media-policy', chatId] }),
  });
  const backfillMedia = useMutation({
    mutationFn: () => api.backfillMedia(chatId),
    onSuccess: () => Promise.all([
      queryClient.invalidateQueries({ queryKey: ['media-progress'] }),
      queryClient.invalidateQueries({ queryKey: ['message-media'] }),
    ]),
  });

  return (
    <footer className="max-h-[30%] shrink-0 overflow-y-auto overscroll-contain border-t bg-card px-3 py-1.5 lg:px-6 lg:py-2.5">
      <div className="flex items-center justify-between gap-2">
        <span className="hidden flex-wrap items-center gap-2 text-xs lg:flex"><ImageIcon aria-hidden="true" className="size-4 text-primary" /><strong>圖片下載</strong><span className="text-muted-foreground">· 預覽圖片會自動下載</span></span>
        <Button className="w-full justify-between lg:hidden" size="sm" variant="ghost" aria-controls={toolsId} aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
          <span className="flex items-center gap-2"><ImageIcon aria-hidden="true" />圖片下載</span><ChevronDown aria-hidden="true" className={expanded ? 'rotate-180' : ''} />
        </Button>
      </div>
      <div id={toolsId} className={`${expanded ? 'flex' : 'hidden'} flex-wrap items-start justify-between gap-x-6 gap-y-2 py-1 lg:flex`}>
        <MediaProgress chatId={chatId} compact />
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <Label className="min-h-8 cursor-pointer text-xs" htmlFor={`${toolsId}-policy`}>
            <Checkbox
              id={`${toolsId}-policy`}
              checked={mediaPolicyDraft ?? policy.data?.auto_archive ?? false}
              disabled={policy.isPending || policy.isError || savePolicy.isPending}
              onCheckedChange={(value) => {
                setMediaPolicyDraft(value === true);
                savePolicy.mutate(value === true);
              }}
            />
            自動下載封存版本
          </Label>
          <Button size="sm" variant="outline" disabled={backfillMedia.isPending} onClick={() => backfillMedia.mutate()}>
            <Download aria-hidden="true" />補下載預覽
          </Button>
        </div>
        <p className="basis-full text-xs text-muted-foreground lg:hidden">預覽圖片會自動下載</p>
        {backfillMedia.isSuccess && <span role="status" className="basis-full text-xs text-primary">已排入預覽補下載</span>}
        <ErrorBox error={policy.error ?? savePolicy.error ?? backfillMedia.error} />
      </div>
    </footer>
  );
}

export default function ChatPane({ chatId }: { chatId: number }) {
  const location = useLocation();
  const searchReturn = searchReturnFrom(location.state);
  const [params, setParams] = useSearchParams();
  const toolsId = useId();
  const [toolsOpen, setToolsOpen] = useState(false);
  const chat = useQuery({ queryKey: ['chat', chatId], queryFn: () => api.chat(chatId) });
  const senders = useQuery({ queryKey: ['senders', chatId], queryFn: () => api.senders(chatId) });

  const messageParam = params.get('message');
  const messageId = messageParam && /^\d+$/.test(messageParam) ? Number(messageParam) : undefined;
  const sender = params.get('sender');
  const date = params.get('date') ?? '';
  const includeDeleted = params.get('deleted') === '1';
  const hideBots = params.get('hide_bots') === '1';

  const update = (key: string, value: string | null) => {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    if (key !== 'message') next.delete('message');
    setParams(next, { state: location.state });
  };

  if (chat.isPending) return <div className="p-5 text-sm text-muted-foreground">載入中…</div>;
  if (chat.error) return <div className="p-4"><ErrorBox error={chat.error} /></div>;
  const c = chat.data;
  const s = c.stats;
  const filters = {
    senderId: sender && /^-?\d+$/.test(sender) ? Number(sender) : undefined,
    toIso: localDayToIso(date, true),
    includeDeleted,
    excludeBots: hideBots,
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="max-h-[45%] shrink-0 overflow-y-auto overscroll-contain border-b bg-card lg:grid lg:grid-cols-[minmax(0,1fr)_auto]">
        {searchReturn && <Link className="mx-4 mt-2 inline-block justify-self-start text-sm text-primary underline-offset-4 hover:underline lg:col-span-2 lg:mx-6" to={searchReturn.to} state={{ searchScrollTop: searchReturn.scrollTop }}>← 返回搜尋結果</Link>}
        <div className="flex flex-wrap items-start justify-between gap-3 px-4 py-3.5 lg:px-6 lg:py-4">
          <div className="min-w-0 flex-1">
            <div className="mb-1.5 flex flex-wrap items-center gap-1.5">
              <Badge variant="outline">{KIND_LABEL[c.kind]}</Badge>
              <Badge variant="secondary" className={c.tracked ? 'bg-primary/10 text-primary' : ''}>
                <span className={`size-1.5 rounded-full ${c.tracked ? 'bg-primary' : 'bg-muted-foreground'}`} />
                {c.tracked ? '收集中' : '未收集'}
              </Badge>
            </div>
            <h2 className="wrap-anywhere text-xl leading-tight font-semibold tracking-tight lg:text-2xl">{c.title || (c.username ? `@${c.username}` : c.id)}</h2>
            <div className="mt-1.5 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-xs text-muted-foreground">
              {c.username && <a className="wrap-anywhere text-primary underline-offset-4 hover:underline" href={`https://t.me/${c.username}`} target="_blank" rel="noreferrer noopener">@{c.username}</a>}
              <span className="font-mono">ID {c.id}</span>
              {s && <span className="inline-flex items-center gap-1 tabular-nums"><MessageSquare aria-hidden="true" className="size-3" />{s.message_count.toLocaleString()} 則訊息</span>}
            </div>
          </div>
          <Button className="lg:hidden" size="sm" variant="outline" aria-expanded={toolsOpen} aria-controls={toolsId} onClick={() => setToolsOpen(!toolsOpen)}>
            <ListFilter aria-hidden="true" />對話工具
          </Button>
        </div>
        <div id={toolsId} className={`${toolsOpen ? 'block' : 'hidden'} lg:contents`}>
          <div className="flex flex-wrap items-start justify-between gap-2 px-4 pb-3 lg:max-w-sm lg:justify-end lg:self-center lg:py-3 lg:pe-6 lg:ps-0">
            <Actions chat={c} />
            {s && (
              <details className="min-w-0 rounded-md text-xs text-muted-foreground">
                <summary className="flex min-h-8 cursor-pointer list-none items-center gap-1.5 rounded-md px-2 font-medium hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring">
                  <Info aria-hidden="true" className="size-4" />封存資訊<ChevronDown aria-hidden="true" className="size-3" />
                </summary>
                <ul className="space-y-1 px-2 py-1.5">
                  <li>已刪除 {s.deleted_count} 則 · {s.history_complete ? '歷史完整' : '歷史未完整'}</li>
                  {s.first_message_at && <li>最早 {formatDateTime(s.first_message_at)}</li>}
                  {s.last_message_at && <li>最新 {formatRelative(s.last_message_at)}</li>}
                  {s.last_sync_completed_at && <li>上次同步 {formatRelative(s.last_sync_completed_at)}</li>}
                </ul>
              </details>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t bg-muted/25 px-4 py-2 lg:col-span-2 lg:px-6">
            <NativeSelect size="sm" className="max-w-48 text-xs" aria-label="寄件人" value={sender ?? ''} onChange={(e) => update('sender', e.target.value || null)}>
              <option value="">所有寄件人</option>
              {senders.data?.map((x) => (
                <option key={x.id} value={x.id}>
                  {x.display_name || (x.username ? `@${x.username}` : x.id)}（{x.message_count}）
                </option>
              ))}
            </NativeSelect>
            <Label htmlFor={`${toolsId}-date`} className="text-xs">
              跳至日期 <Input className="h-8 w-auto min-w-0 text-xs" type="date" id={`${toolsId}-date`} value={date} onChange={(e) => update('date', e.target.value || null)} />
            </Label>
            <Label htmlFor={`${toolsId}-deleted`} className="min-h-8 cursor-pointer text-xs">
              <Checkbox id={`${toolsId}-deleted`} checked={includeDeleted} onCheckedChange={(v) => update('deleted', v === true ? '1' : null)} /> 顯示已刪除
            </Label>
            <Label htmlFor={`${toolsId}-bots`} className="min-h-8 cursor-pointer text-xs">
              <Checkbox id={`${toolsId}-bots`} checked={hideBots} onCheckedChange={(v) => update('hide_bots', v === true ? '1' : null)} /> 隱藏 bot
            </Label>
            <ErrorBox error={senders.error} />
          </div>
        </div>
        {s?.last_error && <p className="mx-4 my-2 flex items-start gap-2 wrap-anywhere text-xs text-destructive lg:col-span-2 lg:mx-6"><CircleAlert aria-hidden="true" className="size-4 shrink-0" />上次同步失敗：{s.last_error}</p>}
        {messageId !== undefined && (
          <div className="flex flex-wrap items-center justify-between gap-2 border-t bg-primary/5 px-4 py-1 text-xs lg:col-span-2 lg:px-6">
            <span className="font-medium text-primary">訊息脈絡 <span className="font-mono">#{messageId}</span></span>
            <Button variant="ghost" size="sm" onClick={() => update('message', null)}>離開脈絡檢視 ✕</Button>
          </div>
        )}
      </header>
      <div className="flex min-h-0 flex-1 flex-col">
        {messageId !== undefined ? (
          <ContextTimeline key={`${messageId}-${includeDeleted}`} chatId={chatId} messageId={messageId} includeDeleted={includeDeleted} chatTitle={c.title} />
        ) : (
          <ListTimeline key={`${sender}-${date}-${includeDeleted}-${hideBots}`} chatId={chatId} filters={filters} chatTitle={c.title} />
        )}
      </div>
      <MediaTools chatId={chatId} />
    </div>
  );
}
