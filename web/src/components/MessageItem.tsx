import { Link, useLocation } from 'react-router-dom';
import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Download, ExternalLink, Hash, Paperclip, RotateCw } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { api, type MediaDownload, type Message } from '../api/client';
import { colorIndex, initials, type GroupPos } from '../lib/bubble';
import { formatDateTime, formatSize, formatTime } from '../lib/format';
import { highlight } from '../lib/highlight';
import { topicLabel } from '../lib/topics';
import BotBadge from './BotBadge';
import SenderLink from './SenderLink';

function MediaPreview({ chatId, messageId, ordinal }: { chatId: number; messageId: number; ordinal: number }) {
  const [visible, setVisible] = useState(false);
  const [brokenPreviewUrl, setBrokenPreviewUrl] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const qc = useQueryClient();
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const observer = new IntersectionObserver(([entry]) => {
      if (entry?.isIntersecting) { setVisible(true); observer.disconnect(); }
    }, { rootMargin: '200px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const mediaKey = ['message-media', chatId, messageId] as const;
  const media = useQuery({
    queryKey: mediaKey,
    queryFn: () => api.messageMedia(chatId, messageId),
    enabled: visible,
    refetchInterval: (query) => query.state.data?.some((item) => ['queued', 'running'].includes(item.state) || (item.state === 'failed' && item.next_attempt_at != null)) ? 1500 : false,
  });
  const preview = media.data?.find((item) => item.ordinal === ordinal && item.variant === 'preview');
  const archive = media.data?.find((item) => item.ordinal === ordinal && item.variant === 'archive');
  const status = useQuery({
    queryKey: ['status'], queryFn: api.status, enabled: preview?.state === 'queued', staleTime: 5000,
  });
  const canPrioritize = preview?.state === 'queued' && !preview.prioritized
    && status.isSuccess && status.data.collector.state !== 'disabled';
  const prioritize = useMutation({
    mutationFn: (id: number) => api.prioritizeMedia(String(id)),
    onSuccess: async (updated) => {
      await qc.cancelQueries({ queryKey: mediaKey });
      qc.setQueryData<MediaDownload[]>(mediaKey, (items) =>
        items?.map((item) => item.id === updated.id ? updated : item));
    },
  });
  const archiveRequest = useMutation({
    mutationFn: () => api.archiveMedia(String(preview!.id)),
    onSuccess: () => void qc.invalidateQueries({ queryKey: mediaKey }),
  });
  const retry = useMutation({
    mutationFn: () => api.retryMedia(String(preview!.id)),
    onSuccess: () => {
      setBrokenPreviewUrl(null);
      void qc.invalidateQueries({ queryKey: mediaKey });
    },
  });
  const canRetryPreview = preview && ['failed', 'unavailable', 'interrupted'].includes(preview.state);
  const canRetryArchive = archive && ['failed', 'unavailable', 'interrupted'].includes(archive.state);
  return (
    <div className="media-preview grid min-w-0 justify-items-start gap-1.5 pt-1 text-xs text-muted-foreground" ref={ref}>
      {(!visible || media.isPending) && <span>圖片預覽…</span>}
      {preview?.state === 'succeeded' && preview.content_url && preview.content_url !== brokenPreviewUrl && (
        <img loading="lazy" src={preview.content_url} width={preview.width ?? undefined} height={preview.height ?? undefined}
          alt="Telegram 圖片預覽" onError={() => setBrokenPreviewUrl(preview.content_url ?? null)} />
      )}
      {preview?.content_url && preview.content_url === brokenPreviewUrl && <span>本地預覽檔目前無法讀取</span>}
      {preview && ['queued', 'running'].includes(preview.state) && <span>預覽{preview.state === 'queued' ? '排隊中' : '下載中'}…</span>}
      {canPrioritize && (
        <Button type="button" size="sm" variant="outline" disabled={prioritize.isPending} onClick={() => prioritize.mutate(preview.id)}>
          {prioritize.isPending ? '正在設定優先下載…' : '優先下載預覽'}
        </Button>
      )}
      {preview?.state === 'queued' && preview.prioritized && <span role="status">已優先排入預覽下載</span>}
      {prioritize.error && <span role="status" className="text-destructive">無法優先下載預覽：{prioritize.error.message}</span>}
      {canRetryPreview && <Button size="sm" variant="outline" disabled={retry.isPending} onClick={() => retry.mutate()}><RotateCw aria-hidden="true" />預覽失敗 · 重試</Button>}
      {!preview && visible && media.isSuccess && <span>預覽尚未排入</span>}
      {media.error && <span role="status" className="text-destructive">無法讀取圖片下載狀態</span>}
      {retry.error && <span role="status" className="text-destructive">無法重試預覽下載</span>}
      {preview && (
        <div className="flex flex-wrap items-center gap-2">
          {archive?.state === 'succeeded' && archive.content_url && (
            <Button size="sm" variant="outline" asChild><a href={archive.content_url} target="_blank" rel="noreferrer"><ExternalLink aria-hidden="true" />查看封存版本</a></Button>
          )}
          {archive && ['queued', 'running'].includes(archive.state) && <span>封存{archive.state === 'queued' ? '排隊中' : '下載中'}…</span>}
          {(!archive || canRetryArchive) && (
            <Button size="sm" variant="outline" disabled={archiveRequest.isPending} onClick={() => archiveRequest.mutate()}>
              {canRetryArchive ? <><RotateCw aria-hidden="true" />封存失敗 · 重試</> : <><Download aria-hidden="true" />下載封存版本</>}
            </Button>
          )}
          {archiveRequest.error && <span role="status" className="text-destructive">{canRetryArchive ? '無法重試封存下載' : '無法排入封存下載'}</span>}
        </div>
      )}
    </div>
  );
}

export function Highlighted({ text, query }: { text: string; query?: string }) {
  if (!query) return <>{text}</>;
  return (
    <>
      {highlight(text, query).map((s, i) => (s.match ? <mark key={i}>{s.text}</mark> : <span key={i}>{s.text}</span>))}
    </>
  );
}

/** display name → @username → chat title (when the sender is the chat itself) → 未知 (id). */
export function senderName(m: Message, chatTitle?: string | null): string {
  const id = m.sender?.id ?? m.sender_id;
  if (m.sender?.display_name) return m.sender.display_name;
  if (m.sender?.username) return `@${m.sender.username}`;
  if (id != null && id === m.chat_id && chatTitle) return chatTitle;
  return id != null ? `未知 (${id})` : '未知';
}

export function forwardName(f: NonNullable<Message['forward']>): string {
  if (f.from_name) return f.from_name;
  return f.from_id != null ? `使用者 ${f.from_id}` : '未知來源';
}

const SINGLE: GroupPos = { first: true, last: true };

export default function MessageItem({
  message: m,
  anchor,
  query,
  showChat,
  chatTitle,
  variant = 'chat',
  pos = SINGLE,
  replyTo,
}: {
  message: Message;
  anchor?: boolean;
  query?: string;
  showChat?: string;
  chatTitle?: string | null;
  /** `chat`: grouped bubbles with avatar; `sender`: fixed sender, shows the chat title instead. */
  variant?: 'chat' | 'sender';
  pos?: GroupPos;
  replyTo?: Message;
}) {
  const location = useLocation();
  const senderVariant = variant === 'sender';
  const id = m.sender?.id ?? m.sender_id;
  const name = senderName(m, chatTitle);
  const self = !senderVariant && m.sender?.is_self === true;
  const system = !m.text && m.attachments.length === 0;
  const showHead = senderVariant || pos.first;
  const cls = [
    'msg',
    m.is_deleted && 'deleted',
    anchor && 'anchor',
    self && 'self',
    system && 'system',
    pos.first && 'first',
    pos.last && 'last',
    senderVariant && 'sender-variant',
    `c${colorIndex(id)}`,
  ]
    .filter(Boolean)
    .join(' ');
  const time = (
    <span className="ms-auto inline-flex items-center gap-1 whitespace-nowrap text-[0.6875rem] text-muted-foreground tabular-nums">
      {m.edited_at && (
        <span title={`編輯於 ${formatDateTime(m.edited_at)}`}>（已編輯）</span>
      )}
      <time dateTime={m.timestamp} title={formatDateTime(m.timestamp)}>
        {formatTime(m.timestamp)}
      </time>
    </span>
  );
  const deleted = m.is_deleted && (
    <Badge variant="outline" className="max-w-full whitespace-normal border-destructive/35 text-destructive" title={m.deleted_at ? formatDateTime(m.deleted_at) : undefined}>
      已刪除{m.deleted_at ? ` ${formatDateTime(m.deleted_at)}` : ''}
    </Badge>
  );
  const nameEl = (
    <strong className="name">
      <SenderLink id={id}>{name}</SenderLink>
    </strong>
  );
  let topicBadge;
  if (!senderVariant && m.topic_id != null) {
    const label = topicLabel(m.topic_title, m.topic_id);
    const search = new URLSearchParams(location.search);
    search.set('topic_id', String(m.topic_id));
    search.delete('message');
    topicBadge = (
      <Badge asChild variant="outline" className="max-w-full whitespace-normal">
        <Link to={{ search: search.toString() }} state={location.state} aria-label={`依話題篩選：${label}`}>
          <Hash aria-hidden="true" className="shrink-0" />
          <span className="min-w-0 wrap-anywhere">{label}</span>
        </Link>
      </Badge>
    );
  }

  if (system) {
    return (
      <article id={`m-${m.id}`} className={cls} aria-current={anchor ? 'true' : undefined}>
        <div className="bubble">
          <header>
            {showChat && (
              <Link className="chat-title" to={`/chats/${m.chat_id}?message=${m.id}${m.is_deleted ? '&deleted=1' : ''}`}>
                {showChat}
              </Link>
            )}
            {nameEl}
            <BotBadge isBot={m.sender?.is_bot} />
            <span className="placeholder text-muted-foreground italic">（系統訊息或無文字內容）</span>
            {topicBadge}
            {deleted}
            {time}
          </header>
        </div>
      </article>
    );
  }

  return (
    <article id={`m-${m.id}`} className={cls} aria-current={anchor ? 'true' : undefined}>
      {!self && !senderVariant && (
        <div className="avatar-col" aria-hidden="true">{pos.last && <span className="avatar">{initials(name)}</span>}</div>
      )}
      <div className="bubble">
        {showHead && (
          <header>
            {showChat && (
              <Link className="chat-title" to={`/chats/${m.chat_id}?message=${m.id}${m.is_deleted ? '&deleted=1' : ''}`}>
                {showChat}
              </Link>
            )}
            {!self && nameEl}
            {self && <span className="text-xs font-semibold text-primary">自己</span>}
            {!self && m.sender?.username && <span className="wrap-anywhere text-xs text-muted-foreground">@{m.sender.username}</span>}
            <BotBadge isBot={m.sender?.is_bot} />
          </header>
        )}
        {m.forward && (
          <p className="my-1 border-s-2 border-primary/40 ps-2 text-xs text-muted-foreground">
            轉發自 {forwardName(m.forward)}
            {m.forward.date && <> · {formatDateTime(m.forward.date)}</>}
          </p>
        )}
        {m.reply_to != null && (
          <Link className="reply" to={`/chats/${m.chat_id}?message=${m.reply_to}`} state={location.state}>
            {replyTo ? (
              <>
                <strong>{senderName(replyTo, chatTitle)}</strong>
                <span className="reply-text">{replyTo.text || '（無文字）'}</span>
                <span className="sr-only">↩ 回覆 #{m.reply_to}</span>
              </>
            ) : (
              <>↩ 回覆 #{m.reply_to}</>
            )}
          </Link>
        )}
        {m.text && (
          <p className="text my-0.5 wrap-anywhere whitespace-pre-wrap">
            <Highlighted text={m.text} query={query} />
          </p>
        )}
        {m.attachments.length > 0 && (
          <ul className="my-1 flex flex-wrap gap-1.5" aria-label="附件">
            {m.attachments.map((a, i) => (
              <li className="min-w-0 max-w-full" key={`${i}:${a.kind}:${a.telegram_file_id}`} aria-label={`附件 ${i + 1}${a.file_name ? `：${a.file_name}` : ''}`}>
                <span className="chip inline-flex max-w-full items-center gap-1 rounded-md border bg-muted/70 px-2 py-1 text-xs">
                  <Paperclip className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
                  <span className="min-w-0 wrap-anywhere">{[a.kind, a.file_name, a.mime_type, formatSize(a.size)].filter(Boolean).join(' · ')}</span>
                </span>
                {!m.is_deleted && (a.kind === 'photo' || (a.kind === 'document' && ['image/jpeg', 'image/png', 'image/webp'].includes(a.mime_type ?? ''))) && (
                  <MediaPreview chatId={m.chat_id} messageId={m.id} ordinal={i} />
                )}
              </li>
            ))}
          </ul>
        )}
        {m.post_author && <p className="post-author my-1 text-xs text-muted-foreground">— {m.post_author}</p>}
        <footer>
          {topicBadge}
          {deleted}
          {time}
        </footer>
      </div>
    </article>
  );
}
