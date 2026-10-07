import type { components } from './schema';

type S = components['schemas'];
export type Chat = S['ChatDto'];
export type ChatKind = S['ChatKindDto'];
export type ChatSort = S['ChatSortDto'];
export type Message = S['MessageDto'];
export type MessagePage = S['MessagePageDto'];
export type MessageContext = S['MessageContextDto'];
export type SenderSummary = S['SenderSummaryDto'];
export type Status = S['StatusDto'];
export type SyncJob = S['SyncJobDto'];
export type TrackResult = S['TrackChatDto'];
export type Attachment = S['AttachmentDto'];
export type MediaDownload = S['MediaDownloadDto'];
export type MediaProgress = S['MediaProgressDto'];
export type MediaPolicy = S['MediaPolicyDto'];
export type SenderProfile = S['SenderProfileDto'];
export type SenderPage = S['SenderPageDto'];
export type SenderDetail = S['SenderDetailDto'];
export type SenderSort = S['SenderSortDto'];

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
    readonly requestId?: string,
  ) {
    super(message);
  }
}

type Query = Record<string, string | number | boolean | null | undefined>;

export function buildQuery(query?: Query): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query ?? {})) {
    if (value !== undefined && value !== null && value !== '') params.set(key, String(value));
  }
  const text = params.toString();
  return text ? `?${text}` : '';
}

async function request<T>(method: string, path: string, query?: Query, jsonBody?: unknown): Promise<T> {
  let response: Response;
  try {
    response = await fetch(path + buildQuery(query), {
      method,
      headers: { accept: 'application/json', ...(jsonBody === undefined ? {} : { 'content-type': 'application/json' }) },
      body: jsonBody === undefined ? undefined : JSON.stringify(jsonBody),
    });
  } catch {
    throw new ApiError('無法連線到伺服器，請確認 tgarchive serve 是否仍在執行', 0, 'network');
  }
  const text = await response.text();
  let body: unknown;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = undefined;
  }
  if (!response.ok) {
    const detail = (body as S['ErrorEnvelope'] | undefined)?.error;
    throw new ApiError(
      detail?.message ?? `HTTP ${response.status}`,
      response.status,
      detail?.code ?? 'http_error',
      detail?.request_id ?? response.headers.get('x-request-id') ?? undefined,
    );
  }
  return body as T;
}

const enc = encodeURIComponent;
const chatPath = (id: number) => `/api/v1/chats/${id}`;

export const api = {
  status: () => request<Status>('GET', '/api/v1/status'),
  syncStatus: () => request<Status>('GET', '/api/v1/sync/status'),
  health: (kind: 'live' | 'ready') => request<S['HealthDto']>('GET', `/health/${kind}`),
  chats: (sort?: ChatSort) => request<Chat[]>('GET', '/api/v1/chats', { sort }),
  chat: (id: number) => request<Chat>('GET', chatPath(id)),
  mediaPolicy: (id: number) => request<MediaPolicy>('GET', `${chatPath(id)}/media-policy`),
  setMediaPolicy: (id: number, auto_archive: boolean) =>
    request<MediaPolicy>('PATCH', `${chatPath(id)}/media-policy`, undefined, { auto_archive }),
  messageMedia: (chatId: number, messageId: number) =>
    request<MediaDownload[]>('GET', `${chatPath(chatId)}/messages/${messageId}/media`),
  media: (id: number) => request<MediaDownload>('GET', `/api/v1/media/${enc(id)}`),
  mediaProgress: () => request<MediaProgress[]>('GET', '/api/v1/media/downloads/status'),
  archiveMedia: (id: string) => request<MediaDownload>('POST', `/api/v1/media/${enc(id)}/archive`),
  retryMedia: (id: string) => request<MediaDownload>('POST', `/api/v1/media/${enc(id)}/retry`),
  backfillMedia: (chatId: number) => request<MediaDownload[]>('POST', `${chatPath(chatId)}/media/downloads`),
  refreshChats: () => request<Chat[]>('POST', '/api/v1/chats/refresh'),
  track: (id: number, backfill: boolean) =>
    request<TrackResult>('PUT', `${chatPath(id)}/tracking`, { backfill: backfill || undefined }),
  untrack: (id: number) => request<Chat>('DELETE', `${chatPath(id)}/tracking`),
  syncChat: (id: number, refetch?: boolean) =>
    request<SyncJob>('POST', `${chatPath(id)}/sync`, { refetch: refetch || undefined }),
  syncAll: () => request<SyncJob>('POST', '/api/v1/sync'),
  syncJob: (id: string) => request<SyncJob>('GET', `/api/v1/sync/jobs/${enc(id)}`),
  senders: (id: number) => request<SenderSummary[]>('GET', `${chatPath(id)}/senders`, { limit: 1000 }),
  senderList: (query: Query) => request<SenderPage>('GET', '/api/v1/senders', query),
  sender: (id: number, includeDeleted?: boolean) =>
    request<SenderDetail>('GET', `/api/v1/senders/${id}`, { include_deleted: includeDeleted || undefined }),
  allMessages: (query: Query) => request<MessagePage>('GET', '/api/v1/messages', query),
  messages: (id: number, query: Query) => request<MessagePage>('GET', `${chatPath(id)}/messages`, query),
  context: (id: number, mid: number, query: Query) =>
    request<MessageContext>('GET', `${chatPath(id)}/messages/${mid}/context`, query),
  search: (query: Query) => request<MessagePage>('GET', '/api/v1/messages/search', query),
};
