import { expect, test } from './fixtures';

const box = (page: import('@playwright/test').Page) => page.getByLabel('搜尋關鍵字');

test('English token: results across chats, highlighted, click opens context', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('kubernetes');
  const results = page.locator('a.result');
  await expect(results).toHaveCount(15);
  await expect(results.first().locator('mark').first()).toHaveText('kubernetes');
  await results.filter({ hasText: 'Rust 台灣社群' }).first().click();
  await expect(page).toHaveURL(/\/chats\/-1002000001\?message=\d+/);
  await expect(page.locator('article.anchor')).toContainText('kubernetes');
});

test('Chinese exact token matches whole tokens only', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('測試關鍵字');
  await expect(page.locator('a.result')).toHaveCount(2);
  await expect(page.locator('a.result mark').first()).toHaveText('測試關鍵字');
  await box(page).fill('關鍵字');
  await expect(page.getByText('找不到符合的訊息')).toBeVisible();
});

test('minimum length hint, chat filter and deleted messages', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('k');
  await expect(page.getByText('請至少輸入 2 個字元')).toBeVisible();
  await box(page).fill('kubernetes');
  await expect(page.locator('a.result')).toHaveCount(15);
  await page.getByLabel('聊天室').selectOption('-1002000002');
  await expect(page.locator('a.result')).toHaveCount(12);
  await page.getByLabel('聊天室').selectOption('');
  await box(page).fill('ephemeral');
  await expect(page.getByText('找不到符合的訊息')).toBeVisible();
  await page.getByLabel('包含已刪除').check();
  const result = page.locator('a.result');
  await expect(result).toHaveCount(1);
  await expect(result.locator('.badge.red')).toContainText('已刪除');
  await result.click();
  await expect(page).toHaveURL(/message=60&deleted=1/);
  await expect(page.locator('article.anchor.deleted')).toBeVisible();
});

for (const width of [360, 768, 1400, 1600]) {
  test(`filter bar stays tidy at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto('/search');
    const bar = page.locator('.search-filters');
    await expect(bar).toBeVisible();
    const fits = await bar.evaluate((el) =>
      [...el.querySelectorAll('select, input, label')].every((c) => {
        const r = c.getBoundingClientRect();
        return r.left >= 0 && r.right <= window.innerWidth + 0.5;
      }),
    );
    expect(fits).toBe(true);
    const [from, to] = await page.locator('.date-range input').evaluateAll((els) =>
      els.map((e) => e.getBoundingClientRect().top),
    );
    if (width >= 1400) expect(Math.abs((from ?? 0) - (to ?? 1))).toBeLessThan(2);
    // "到" stays on the same line as its own date input.
    const label = await page.locator('.date-range label').nth(1).evaluate((el) => {
      const r = el.getBoundingClientRect();
      const i = el.querySelector('input')!.getBoundingClientRect();
      return Math.abs(r.top - i.top) < 12;
    });
    expect(label).toBe(true);
  });
}
