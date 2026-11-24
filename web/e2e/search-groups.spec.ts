import { expect, test } from './fixtures';

test('saved groups persist, restrict results, and can be edited or deleted without altering messages', async ({ page, request }) => {
  await page.goto('/search?q=kubernetes');
  const results = page.locator('a.result');
  const manage = page.getByRole('button', { name: '管理搜尋群組' });
  const save = page.getByRole('button', { name: '儲存群組' });
  await expect(results).toHaveCount(15);
  await manage.click();
  await page.getByRole('button', { name: '新增群組' }).click();
  await page.getByLabel('群組名稱').fill('技術選讀');
  await page.getByRole('checkbox', { name: /Rust 台灣社群/ }).check();
  await save.click();
  await expect(results).toHaveCount(3);
  const saved = page.url();
  const id = new URL(saved).searchParams.get('group_id');
  expect(id).toMatch(/^\d+$/);
  try {
    await page.reload();
    await expect(page.getByLabel('搜尋範圍')).toHaveValue(id!);
    await expect(results).toHaveCount(3);
    await manage.click();
    await page.getByRole('button', { name: /編輯.*技術選讀/ }).click();
    await page.getByLabel('群組名稱').fill('新聞選讀');
    await page.getByRole('checkbox', { name: /Rust 台灣社群/ }).uncheck();
    await page.getByRole('checkbox', { name: /Daily News/ }).check();
    await save.click();
    await expect(results).toHaveCount(12);
    await expect(results.filter({ hasText: 'Rust 台灣社群' })).toHaveCount(0);
    await manage.click();
    await page.getByRole('button', { name: /編輯.*新聞選讀/ }).click();
    await page.getByRole('button', { name: '刪除群組', exact: true }).click();
    await page.getByRole('button', { name: '確認刪除' }).click();
    await expect(results).toHaveCount(15);
    await page.goto(saved);
    await expect(page.getByRole('alert')).toContainText('404');
    await expect(results).toHaveCount(0);
  } finally {
    await request.delete(`/api/v1/search-groups/${id}`);
  }
});
