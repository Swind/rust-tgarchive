import { Bot } from 'lucide-react';
import { Badge } from '@/components/ui/badge';

export default function BotBadge({ isBot }: { isBot?: boolean | null }) {
  if (isBot !== true) return null;
  return (
    <Badge variant="secondary" className="gap-1 px-1.5 align-middle" role="img" aria-label="Bot" title="機器人帳號">
      <Bot aria-hidden="true" />Bot
    </Badge>
  );
}
