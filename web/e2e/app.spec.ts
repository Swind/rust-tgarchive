import { CHAT, expect, test } from './fixtures';

test('app loads, redirects to chats and the nav works', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveURL(/\/chats$/);
  const nav = page.getByRole('navigation', { name: '主選單' });
  for (const [name, url] of [['搜尋', /\/search$/], ['同步', /\/sync$/], ['狀態', /\/status$/], ['對話', /\/chats$/]] as const) {
    await nav.getByRole('link', { name }).click();
    await expect(page).toHaveURL(url);
    await expect(nav.getByRole('link', { name })).toHaveClass(/active/);
  }
  await page.goto('/nope');
  await expect(page.getByText('找不到頁面')).toBeVisible();
});

test('collector badge shows disabled with an explanatory tooltip', async ({ page }) => {
  await page.goto('/chats');
  const badge = page.locator('.topbar .badge', { hasText: '收集器：' });
  await expect(badge).toContainText('disabled');
  await expect(badge).toHaveAttribute('title', /query-only/);
});

test('theme toggle persists across reload and applies dark styles', async ({ page }) => {
  await page.goto('/chats');
  const bg = () => page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(await bg()).toBe('rgb(255, 255, 255)');
  await page.getByRole('button', { name: '切換為深色主題' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  expect(await bg()).toBe('rgb(22, 24, 29)');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  expect(await bg()).toBe('rgb(22, 24, 29)');
  await page.getByRole('button', { name: '切換為淺色主題' }).click();
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
});

test('status page shows collector, unresolved deletions and health', async ({ page }) => {
  await page.goto('/status');
  const detail = page.locator('dl.detail').first();
  await expect(detail).toContainText('disabled');
  await expect(detail).toContainText('query-only');
  await expect(detail.locator('dt', { hasText: '未歸屬刪除' }).locator('+ dd')).toContainText(/^1（/);
  await expect(detail.locator('dt', { hasText: '同步工作' }).locator('+ dd')).toHaveText('4');
  await expect(page.locator('dt', { hasText: /^live$/ }).locator('+ dd')).toHaveText('live');
  await expect(page.locator('dt', { hasText: /^ready$/ }).locator('+ dd')).toHaveText('ready');
});

test('unknown chat renders the API error with its request id', async ({ page }) => {
  await page.goto('/chats/-999');
  const alert = page.getByRole('alert');
  await expect(alert).toContainText('not_found');
  await expect(alert).toContainText('HTTP 404');
  await expect(alert).toContainText(/request_id: \S+/);
  await page.goto('/chats/abc');
  await expect(page.getByText('無效的聊天室 ID')).toBeVisible();
});

test('tracking changes in query-only mode show a friendly error and keep state', async ({ page }) => {
  await page.goto(`/chats/${CHAT.family}`);
  await expect(page.getByRole('heading', { level: 2 })).toContainText('家庭群組');
  page.once('dialog', (d) => void d.accept());
  await page.getByRole('button', { name: '停止收集' }).click();
  await expect(page.getByRole('alert')).toContainText('storage_unavailable');
  await expect(page.getByRole('button', { name: '停止收集' })).toBeVisible();
});

test('sync and track on an untracked chat report errors without a collector', async ({ page }) => {
  await page.goto(`/chats/${CHAT.empty}`);
  await page.getByRole('button', { name: '同步', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('busy');
  await page.getByRole('button', { name: '開始收集' }).click();
  await expect(page.getByRole('alert')).toContainText('storage_unavailable');
  await expect(page.getByRole('button', { name: '開始收集' })).toBeVisible();
});
