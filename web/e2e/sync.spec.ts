import { CHAT, expect, test } from './fixtures';
import type { SyncJob } from '../src/api/client';

const job = (page: import('@playwright/test').Page, state: string) => page.locator(`li.job[data-state="${state}"]`);
const syncJob = (id: string, scope: string, state: string): SyncJob => ({
  id, scope, state, created_at: '2026-03-12T05:00:00Z', has_error: state === 'failed',
});

test('lists every job with its error summary and retry-after hint', async ({ page }) => {
  await page.goto('/sync');
  await expect(page.locator('li.job')).toHaveCount(4);
  await expect(job(page, 'failed')).toContainText('Telegram unavailable: connection reset by peer');
  await expect(job(page, 'rate_limited')).toContainText('retry after 3600 s');
  await expect(job(page, 'rate_limited')).toContainText('約 3600 秒後可重試');
  await expect(job(page, 'interrupted')).toContainText('process stopped before job completed');
  await expect(job(page, 'succeeded')).not.toContainText('⚠️');
});

test('retry is offered for failed/interrupted chat jobs and reports a friendly error in query-only mode', async ({ page }) => {
  await page.goto('/sync');
  await expect(job(page, 'succeeded').getByRole('button', { name: '重試', exact: true })).toHaveCount(0);
  await expect(job(page, 'rate_limited').getByRole('button', { name: '重試', exact: true })).toHaveCount(0);
  await job(page, 'failed').getByRole('button', { name: '重試', exact: true }).click();
  await expect(job(page, 'failed').getByRole('alert')).toContainText('沒有執行中的 Telegram 收集器');
  await expect(job(page, 'interrupted').getByRole('button', { name: '重試', exact: true })).toBeVisible();
});

test('expanding a job shows per-chat progress', async ({ page }) => {
  await page.goto('/sync');
  const limited = job(page, 'rate_limited');
  await limited.locator('button.job-head').click();
  const items = limited.locator('.chat-progress li');
  await expect(items).toHaveCount(2);
  await expect(items.filter({ hasText: '家庭群組' })).toContainText('已寫入 14 則');
  const old = items.filter({ hasText: '舊專案群組' });
  await expect(old).toContainText('已寫入 40 則');
  await expect(old).toContainText('retry after 3600 s');
  await expect(old.getByRole('button', { name: '重試', exact: true })).toBeVisible();
  await expect(items.filter({ hasText: '家庭群組' }).getByRole('button', { name: '重試', exact: true })).toHaveCount(0);
  await limited.locator('button.job-head').click();
  await expect(items).toHaveCount(0);
});

test('sync all reports that no collector is running', async ({ page }) => {
  await page.goto('/sync');
  await page.getByRole('button', { name: '同步全部' }).click();
  await expect(page.getByRole('alert').first()).toContainText('沒有執行中的 Telegram 收集器');
});

test('chat sync blocks its active job, then becomes available when that job completes', async ({ page }) => {
  let active = true;
  let posted = false;
  await page.route('**/api/v1/sync/status', (route) => route.fulfill({
    json: { sync_jobs: active ? [{ id: 'active', scope: `chat:${CHAT.alice}`, state: 'running' }] : [] },
  }));
  await page.route(`**/api/v1/chats/${CHAT.alice}/sync`, (route) => {
    posted = true;
    active = true;
    return route.fulfill({ status: 202, json: { id: 'new-job', scope: `chat:${CHAT.alice}`, state: 'queued' } });
  });
  await page.goto(`/chats/${CHAT.alice}`);
  const sync = page.getByRole('button', { name: '同步', exact: true });
  await expect(sync).toBeDisabled();
  await expect(page.getByRole('status')).toContainText('已有同步工作');
  expect(posted).toBe(false);

  active = false;
  await expect(sync).toBeEnabled({ timeout: 7000 });
  await sync.click();
  await expect(page.getByText('已建立同步工作')).toBeVisible();
  await expect(sync).toBeDisabled();
  expect(posted).toBe(true);
  active = false;
  await expect(sync).toBeEnabled({ timeout: 7000 });
});

test('chat sync handles a conflict race with a localized message and request id', async ({ page }) => {
  let statusCalls = 0;
  let conflict = false;
  await page.route('**/api/v1/sync/status', (route) => {
    statusCalls++;
    return route.fulfill({ json: { sync_jobs: conflict ? [{ id: 'raced', scope: `chat:${CHAT.alice}`, state: 'running' }] : [] } });
  });
  await page.route(`**/api/v1/chats/${CHAT.alice}/sync`, (route) => route.fulfill({
    status: 409,
    json: { error: { code: 'conflict', message: 'Operation conflicts with current state', request_id: 'race-123' } },
  }));
  await page.goto(`/chats/${CHAT.alice}`);
  await expect(page.getByRole('button', { name: '同步', exact: true })).toBeEnabled();
  const before = statusCalls;
  conflict = true;
  await page.getByRole('button', { name: '同步', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('已有同步工作正在進行');
  await expect(page.getByRole('alert')).toContainText('race-123');
  await expect.poll(() => statusCalls).toBeGreaterThan(before);
  await expect(page.getByRole('button', { name: '同步', exact: true })).toBeDisabled();
});

test('an active job for another chat only blocks sync all', async ({ page }) => {
  await page.route('**/api/v1/sync/status', (route) => route.fulfill({
    json: { sync_jobs: [syncJob('other', `chat:${CHAT.news}`, 'running')] },
  }));
  await page.goto('/sync');
  await expect(page.locator('li.job[data-state="running"]')).toBeVisible();
  await expect(page.getByRole('button', { name: '同步全部' })).toBeDisabled();
  await page.goto(`/chats/${CHAT.alice}`);
  await expect(page.getByRole('button', { name: '同步', exact: true })).toBeEnabled();
});

test('an active all job blocks chat sync and retries, and sync all unlocks on completion', async ({ page }) => {
  let active = true;
  await page.route('**/api/v1/sync/status', (route) => route.fulfill({
    json: { sync_jobs: [
      ...(active ? [syncJob('all-job', 'all', 'running')] : []),
      syncJob('failed-chat', `chat:${CHAT.alice}`, 'failed'),
    ] },
  }));
  await page.goto('/sync');
  const syncAll = page.getByRole('button', { name: '同步全部' });
  await expect(page.locator('li.job[data-state="running"]').filter({ hasText: 'all' })).toBeVisible();
  await expect(syncAll).toBeDisabled();
  const failed = job(page, 'failed');
  await expect(failed.getByRole('button', { name: '重試', exact: true })).toBeDisabled();
  await page.goto(`/chats/${CHAT.alice}`);
  await expect(page.getByRole('button', { name: '同步', exact: true })).toBeDisabled();
  await page.goto('/sync');
  await expect(syncAll).toBeDisabled();
  active = false;
  await expect(syncAll).toBeEnabled({ timeout: 7000 });
  await expect(failed.getByRole('button', { name: '重試', exact: true })).toBeEnabled();
});
