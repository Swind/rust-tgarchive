import { useLayoutEffect, useRef, type MouseEvent } from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';
import { ChevronRight, Hash, MessageSquare } from 'lucide-react';
import type { Message } from '@/api/client';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { formatDateTime } from '@/lib/format';
import { topicLabel } from '@/lib/topics';
import BotBadge from './BotBadge';
import { Highlighted, senderName } from './MessageItem';
import SenderLink from './SenderLink';

type SearchReturn = { to: string; scrollTop: number };

export function searchReturnFrom(state: unknown): SearchReturn | undefined {
  const value = (state as { searchReturn?: SearchReturn } | null)?.searchReturn;
  if (value && typeof value.to === 'string' && /^\/(?:search(?:\?|$)|senders\/-?\d+\?)/.test(value.to) &&
    Number.isFinite(value.scrollTop) && value.scrollTop >= 0) return value;
}

/** Restore only once the cached results (or their first fetch) are rendered. */
export function useRestoreSearchScroll(ready: boolean) {
  const location = useLocation();
  const restored = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (!ready || restored.current === location.key) return;
    restored.current = location.key;
    const top: unknown = location.state?.searchScrollTop;
    const main = document.querySelector('main');
    if (main && typeof top === 'number' && Number.isFinite(top) && top >= 0) main.scrollTop = top;
  }, [ready, location.key, location.state]);
}

export default function SearchResultLink({ message: m, query, chatTitle, includeDeleted, showSender, onExclude, topicTo }: {
  message: Message; query: string; chatTitle?: string; includeDeleted: boolean; showSender?: boolean; onExclude?: () => void; topicTo?: string;
}) {
  const location = useLocation();
  const navigate = useNavigate();
  const to = `/chats/${m.chat_id}?message=${m.id}${includeDeleted ? '&deleted=1' : ''}`;
  const topic = m.topic_id == null ? null : topicLabel(m.topic_title, m.topic_id);
  const topicContent = topic && <>{m.topic_title && <Hash aria-hidden="true" className="shrink-0" />}<span className="truncate">{topic}</span></>;
  function openContext(event: MouseEvent<HTMLAnchorElement>) {
    // Keep native open-in-new-tab/window behaviour for modified clicks.
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    const scrollTop = document.querySelector('main')?.scrollTop ?? 0;
    // Store on both entries: browser Back and the explicit return link restore the same position.
    void navigate(location.pathname + location.search, { replace: true, state: { ...location.state, searchScrollTop: scrollTop } });
    void navigate(to, { state: { searchReturn: { to: location.pathname + location.search, scrollTop } } });
  }
  return (
    <li className="group relative grid min-w-0 gap-2 px-4 py-4 hover:bg-muted/45 sm:px-5">
      <Link to={to} onClick={openContext} className="result grid min-w-0 gap-2 rounded-sm outline-offset-2 after:absolute after:inset-0 focus-visible:outline-2 focus-visible:outline-ring">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
          <MessageSquare className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
          <strong className="min-w-0 break-all text-sm font-semibold text-foreground">{chatTitle ?? m.chat_id}</strong>
          {m.is_deleted && <Badge variant="destructive">已刪除</Badge>}
          <time dateTime={m.timestamp} className="tabular-nums sm:ms-auto">{formatDateTime(m.timestamp)}</time>
          <ChevronRight aria-hidden="true" className="hidden size-4 sm:block" />
        </div>
        <p className="text wrap-anywhere whitespace-pre-wrap text-sm leading-6 [&_mark]:rounded-sm [&_mark]:bg-primary/15 [&_mark]:px-0.5 [&_mark]:font-semibold [&_mark]:text-foreground">
          <Highlighted text={m.snippet ?? m.text ?? ''} query={query} />
        </p>
      </Link>
      {(showSender || topic) && <div className="flex min-w-0 flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
        {topic && <Badge asChild={!!topicTo} variant="outline" className="max-w-full min-w-0">
          {topicTo ? <Link to={topicTo} className="relative z-1 min-h-6 pointer-coarse:min-h-10" aria-label={`${topic}（${chatTitle ?? m.chat_id}）`}>{topicContent}</Link> : topicContent}
        </Badge>}
        {showSender && <>
          <span className="relative z-1 min-w-0 break-all"><SenderLink id={m.sender?.id ?? m.sender_id}>{senderName(m, chatTitle)}</SenderLink></span>
          <BotBadge isBot={m.sender?.is_bot} />
          {onExclude && <Button type="button" variant="ghost" size="sm" className="relative z-1 ms-auto" aria-label={`排除來源 ${chatTitle ?? m.chat_id}`} onClick={onExclude}>排除此來源</Button>}
        </>}
      </div>}
    </li>
  );
}
