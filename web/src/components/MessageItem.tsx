import { Link } from 'react-router-dom';
import type { Message } from '../api/client';
import { colorIndex, initials, type GroupPos } from '../lib/bubble';
import { formatDateTime, formatSize, formatTime } from '../lib/format';
import { highlight } from '../lib/highlight';
import BotBadge from './BotBadge';
import SenderLink from './SenderLink';

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

function Avatar({ id, name }: { id: number | null | undefined; name: string }) {
  return (
    <div className={`avatar c${colorIndex(id)}`} role="img" aria-label={name}>
      {initials(name)}
    </div>
  );
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
    <span className="meta">
      {m.edited_at && (
        <span title={`編輯於 ${formatDateTime(m.edited_at)}`}>（已編輯）</span>
      )}
      <time dateTime={m.timestamp} title={formatDateTime(m.timestamp)}>
        {formatTime(m.timestamp)}
      </time>
    </span>
  );
  const deleted = m.is_deleted && (
    <span className="badge red" title={m.deleted_at ? formatDateTime(m.deleted_at) : undefined}>
      已刪除{m.deleted_at ? ` ${formatDateTime(m.deleted_at)}` : ''}
    </span>
  );
  const nameEl = (
    <strong className="name">
      <SenderLink id={id}>{name}</SenderLink>
    </strong>
  );

  if (system) {
    return (
      <article id={`m-${m.id}`} className={cls} aria-current={anchor ? 'true' : undefined}>
        <div className="bubble pill-bubble">
          <header>
            {showChat && (
              <Link className="chat-title" to={`/chats/${m.chat_id}?message=${m.id}${m.is_deleted ? '&deleted=1' : ''}`}>
                {showChat}
              </Link>
            )}
            {nameEl}
            <BotBadge isBot={m.sender?.is_bot} />
            <span className="placeholder muted">（系統訊息或無文字內容）</span>
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
        <div className="avatar-col">{pos.last && <Avatar id={id} name={name} />}</div>
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
            {!self && m.sender?.username && <span className="muted small username">@{m.sender.username}</span>}
            <BotBadge isBot={m.sender?.is_bot} />
          </header>
        )}
        {m.forward && (
          <p className="forward muted small">
            轉發自 {forwardName(m.forward)}
            {m.forward.date && <> · {formatDateTime(m.forward.date)}</>}
          </p>
        )}
        {m.reply_to != null && (
          <Link className="reply" to={`/chats/${m.chat_id}?message=${m.reply_to}`}>
            {replyTo ? (
              <>
                <strong>{senderName(replyTo, chatTitle)}</strong>
                <span className="reply-text">{replyTo.text || '（無文字）'}</span>
                <span className="visually-hidden">↩ 回覆 #{m.reply_to}</span>
              </>
            ) : (
              <>↩ 回覆 #{m.reply_to}</>
            )}
          </Link>
        )}
        {m.text && (
          <p className="text">
            <Highlighted text={m.text} query={query} />
          </p>
        )}
        {m.attachments.length > 0 && (
          <ul className="chips">
            {m.attachments.map((a, i) => (
              <li key={i} className="chip">
                {[a.kind, a.file_name, a.mime_type, formatSize(a.size)].filter(Boolean).join(' · ')}
              </li>
            ))}
          </ul>
        )}
        {m.post_author && <p className="post-author muted small">— {m.post_author}</p>}
        <footer>
          {deleted}
          {time}
        </footer>
      </div>
    </article>
  );
}
