import { Link } from 'react-router-dom';
import type { Message } from '../api/client';
import { formatDateTime, formatSize, formatTime } from '../lib/format';
import { highlight } from '../lib/highlight';

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

export default function MessageItem({
  message: m,
  anchor,
  query,
  showChat,
  chatTitle,
}: {
  message: Message;
  anchor?: boolean;
  query?: string;
  showChat?: string;
  chatTitle?: string | null;
}) {
  return (
    <article
      id={`m-${m.id}`}
      className={`msg${m.is_deleted ? ' deleted' : ''}${anchor ? ' anchor' : ''}`}
      aria-current={anchor ? 'true' : undefined}
    >
      <header>
        {showChat && <span className="chat-title">{showChat}</span>}
        <strong>{senderName(m, chatTitle)}</strong>
        <time dateTime={m.timestamp} title={formatDateTime(m.timestamp)}>
          {formatTime(m.timestamp)}
        </time>
        {m.edited_at && <span className="muted" title={`編輯於 ${formatDateTime(m.edited_at)}`}>（已編輯）</span>}
        {m.is_deleted && (
          <span className="badge red" title={m.deleted_at ? formatDateTime(m.deleted_at) : undefined}>
            已刪除{m.deleted_at ? ` ${formatDateTime(m.deleted_at)}` : ''}
          </span>
        )}
        {m.reply_to != null && (
          <Link className="reply" to={`/chats/${m.chat_id}?message=${m.reply_to}`}>
            ↩ 回覆 #{m.reply_to}
          </Link>
        )}
      </header>
      {m.text && (
        <p className="text">
          <Highlighted text={m.text} query={query} />
        </p>
      )}
      {!m.text && m.attachments.length === 0 && <p className="placeholder muted">（系統訊息或無文字內容）</p>}
      {m.attachments.length > 0 && (
        <ul className="chips">
          {m.attachments.map((a, i) => (
            <li key={i} className="chip">
              {[a.kind, a.file_name, a.mime_type, formatSize(a.size)].filter(Boolean).join(' · ')}
            </li>
          ))}
        </ul>
      )}
    </article>
  );
}
