import type { SenderProfile } from '../api/client';

type Named = { id: number; display_name?: string | null; username?: string | null };

/** display name → @username → 未知 (id). */
export function senderLabel(s: Named): string {
  return s.display_name || (s.username ? `@${s.username}` : `未知 (${s.id})`);
}

export const KIND_SENDER: Record<SenderProfile['kind'], string> = {
  user: '使用者',
  chat: '群組',
  channel: '頻道',
  unknown: '未知',
};
