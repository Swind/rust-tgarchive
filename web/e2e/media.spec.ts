import { CHAT, expect, test } from './fixtures';

const image = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=', 'base64');

test('previews are local and lazy, archive is explicit, and channel policy saves', async ({ page }) => {
  const base = `**/api/v1/chats/${CHAT.big}`;
  let autoArchive = false;
  let archiveState = 0;
  await page.route(`${base}/media-policy`, async (route) => {
    if (route.request().method() === 'PATCH') autoArchive = (route.request().postDataJSON() as { auto_archive: boolean }).auto_archive;
    await route.fulfill({ json: { auto_archive: autoArchive } });
  });
  await page.route(`${base}/messages/30/media`, async (route) => {
    const preview = { id: 501, ordinal: 0, variant: 'preview', state: 'succeeded', width: 1, height: 1, byte_size: image.length, content_url: '/api/v1/media/501/content' };
    const archive = archiveState > 0 ? {
      id: 502, ordinal: 0, variant: 'archive',
      state: archiveState === 1 ? 'queued' : archiveState === 2 ? 'running' : 'succeeded',
      width: 1, height: 1, byte_size: image.length,
      content_url: archiveState >= 3 ? '/api/v1/media/502/content' : null,
    } : null;
    if (archiveState > 0 && archiveState < 3) archiveState++;
    await route.fulfill({ json: [preview, ...(archive ? [archive] : [])] });
  });
  await page.route('**/api/v1/media/501/content', (route) => route.fulfill({ status: 200, contentType: 'image/png', body: image }));
  await page.route('**/api/v1/media/502/content', (route) => route.fulfill({ status: 200, contentType: 'image/png', body: image }));
  await page.route('**/api/v1/media/501/archive', async (route) => {
    archiveState = 1;
    await route.fulfill({ status: 202, json: { id: 502, ordinal: 0, variant: 'archive', state: 'queued', content_url: null } });
  });

  await page.goto(`/chats/${CHAT.big}?message=30`);
  await expect(page.getByLabel('自動下載封存版本')).toBeVisible();
  await page.getByLabel('自動下載封存版本').check();
  await expect.poll(() => autoArchive).toBe(true);

  const item = page.locator('#m-30');
  await item.scrollIntoViewIfNeeded();
  await expect(item.getByAltText('Telegram 圖片預覽')).toBeVisible();
  await item.getByRole('button', { name: '下載封存版本' }).click();
  await expect(item.getByText('封存排隊中…')).toBeVisible();
  await expect(item.getByText('封存下載中…')).toBeVisible();
  await expect(item.getByRole('link', { name: '查看封存版本' })).toBeVisible();
});

test('a cancelled automatic archive can be requested manually', async ({ page }) => {
  let requested = false;
  const base = `**/api/v1/chats/${CHAT.big}`;
  await page.route(`${base}/media-policy`, (route) => route.fulfill({ json: { auto_archive: false } }));
  await page.route(`${base}/messages/30/media`, (route) => route.fulfill({ json: [
    { id: 601, ordinal: 0, variant: 'preview', state: 'succeeded', content_url: '/api/v1/media/601/content' },
    { id: 602, ordinal: 0, variant: 'archive', state: requested ? 'succeeded' : 'interrupted', content_url: requested ? '/api/v1/media/602/content' : null },
  ] }));
  await page.route('**/api/v1/media/601/content', (route) => route.fulfill({ contentType: 'image/png', body: image }));
  await page.route('**/api/v1/media/601/archive', (route) => {
    requested = true;
    return route.fulfill({ status: 202, json: { id: 602, ordinal: 0, variant: 'archive', state: 'queued', content_url: null } });
  });
  await page.goto(`/chats/${CHAT.big}?message=30`);
  const item = page.locator('#m-30');
  await item.scrollIntoViewIfNeeded();
  await item.getByRole('button', { name: '封存失敗 · 重試' }).click();
  await expect(item.getByRole('link', { name: '查看封存版本' })).toBeVisible();
  expect(requested).toBe(true);
  await expect(page.getByLabel('自動下載封存版本')).not.toBeChecked();
});
