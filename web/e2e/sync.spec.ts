import { expect, test } from './fixtures';

const job = (page: import('@playwright/test').Page, state: string) => page.locator(`li.job[data-state="${state}"]`);

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
