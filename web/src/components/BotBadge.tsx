/** 🤖 marker for senders Telegram flags as bots (`is_bot === true`). */
export default function BotBadge({ isBot }: { isBot?: boolean | null }) {
  if (isBot !== true) return null;
  return (
    <span className="badge blue bot-badge" role="img" aria-label="Bot" title="Bot">
      🤖
    </span>
  );
}
