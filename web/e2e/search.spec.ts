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

test('Chinese words and substrings are found (jieba words + character bigrams)', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('測試關鍵字');
  await expect(page.locator('a.result')).toHaveCount(2);
  await expect(page.locator('a.result mark').first()).toHaveText('測試關鍵字');
  // Substring of a word: the old whole-token search could not find this.
  await box(page).fill('關鍵字');
  await expect(page.locator('a.result')).toHaveCount(2);
  await box(page).fill('伺服器');
  await expect(page.locator('a.result')).toHaveCount(1);
  await expect(page.locator('a.result').first()).toContainText('伺服器昨晚又當機了');
  await box(page).fill('硬碟');
  await expect(page.locator('a.result')).toHaveCount(2);
});

test('character-sequence match is contiguous, not scattered', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('北咖啡');
  const results = page.locator('a.result');
  await expect(results).toHaveCount(2);
  await expect(results.filter({ hasText: '我在新北咖啡店工作' })).toHaveCount(1);
  await expect(results.filter({ hasText: '台北咖啡廳的拿鐵很好喝' })).toHaveCount(1);
  await expect(results.first().locator('mark').first()).toHaveText('北咖啡');
  // 台北車站附近的咖啡很貴 / 今天下午要去台北喝咖啡 have the characters but not 北咖啡 in sequence.
  await expect(results.filter({ hasText: '車站' })).toHaveCount(0);
  await expect(results.filter({ hasText: '去台北喝咖啡' })).toHaveCount(0);
  // The word query 台北咖啡 still finds the scattered ones.
  await box(page).fill('台北咖啡');
  await expect(results.filter({ hasText: '台北車站附近的咖啡很貴' })).toHaveCount(1);
  await expect(results.filter({ hasText: '我在新北咖啡店工作' })).toHaveCount(0);
});

test('mixed-language and fullwidth queries', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('GitLab Runner');
  await expect(page.locator('a.result')).toHaveCount(1);
  await expect(page.locator('a.result')).toContainText('沒有回應');
  await box(page).fill('chromium');
  await expect(page.locator('a.result')).toHaveCount(1);
  await box(page).fill('sqlite');
  await expect(page.locator('a.result')).toHaveCount(1);
  await expect(page.locator('a.result')).toContainText('ＳＱＬｉｔｅ');
});

test('single-character query scans text and shows the slow-mode banner', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('北');
  await expect(page.getByRole('status').filter({ hasText: '單字查詢' })).toBeVisible();
  const results = page.locator('a.result');
  await expect(results.first()).toBeVisible();
  // 北 at the end of 台北 is found.
  await expect(results.filter({ hasText: '台北咖啡廳的拿鐵很好喝' })).toHaveCount(1);
  await box(page).fill('北咖啡');
  await expect(page.getByRole('status').filter({ hasText: '單字查詢' })).toHaveCount(0);
});

test('sort toggle switches between relevance and time order', async ({ page }) => {
  await page.goto('/search');
  const relevance = page.getByRole('button', { name: '相關性', exact: true });
  const time = page.getByRole('button', { name: '時間', exact: true });
  await expect(relevance).toHaveAttribute('aria-pressed', 'true');
  const requested = page.waitForRequest((r) => r.url().includes('/messages/search') && r.url().includes('sort=relevance'));
  await box(page).fill('咖啡');
  await requested;
  await expect(page.locator('a.result')).toHaveCount(4);
  const timeRequest = page.waitForRequest((r) => r.url().includes('sort=time'));
  await time.click();
  await timeRequest;
  await expect(time).toHaveAttribute('aria-pressed', 'true');
  await expect(relevance).toHaveAttribute('aria-pressed', 'false');
  // Newest first: the last evaluation message (9th, 新北咖啡店) comes before the first one.
  const results = page.locator('a.result');
  await expect(results.first()).toContainText('新北咖啡店');
  await expect(results).toHaveCount(4);
  await expect(results.last()).toContainText('台北喝咖啡');
});

test('long messages show a snippet around the match', async ({ page }) => {
  await page.goto('/search');
  await box(page).fill('硬碟');
  const long = page.locator('a.result', { hasText: '最後提到' });
  await expect(long).toHaveCount(1);
  const text = (await long.locator('.text').textContent()) ?? '';
  expect(text.startsWith('…')).toBe(true);
  expect(text.length).toBeLessThanOrEqual(125);
  await expect(long.locator('mark').first()).toHaveText('硬碟');
  const short = page.locator('a.result', { hasText: '硬碟壞軌需要更換' });
  await expect(short.locator('.text')).toHaveText('硬碟壞軌需要更換');
});

test('empty hint, chat filter and deleted messages', async ({ page }) => {
  await page.goto('/search');
  await expect(page.getByText('輸入關鍵字開始搜尋')).toBeVisible();
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

test('URL holds search state: deep link, filters, back navigation', async ({ page }) => {
  await page.goto('/search?q=北咖啡&sort=time');
  await expect(box(page)).toHaveValue('北咖啡');
  await expect(page.getByRole('button', { name: '時間', exact: true })).toHaveAttribute('aria-pressed', 'true');
  const results = page.locator('a.result');
  await expect(results).toHaveCount(2);
  await page.getByRole('button', { name: '相關性', exact: true }).click();
  await expect(page).not.toHaveURL(/sort=/);
  await page.getByRole('button', { name: '時間', exact: true }).click();
  await page.getByLabel('包含已刪除').check();
  await expect(page).toHaveURL(/sort=time/);
  await expect(page).toHaveURL(/include_deleted=1/);
  await page.getByLabel('聊天室').selectOption('-1002000002');
  await expect(page).toHaveURL(/chat=-1002000002/);
  await page.getByLabel('聊天室').selectOption('');
  await box(page).fill('咖啡');
  await expect(page).toHaveURL(/q=%E5%92%96%E5%95%A1(&|$)/);
  await expect(results).toHaveCount(4);
  await results.first().click();
  await expect(page).toHaveURL(/\/chats\//);
  await page.goBack();
  await expect(page).toHaveURL(/\/search\?.*sort=time/);
  await expect(box(page)).toHaveValue('咖啡');
  await expect(results).toHaveCount(4);
  await page.goBack();
  await expect(page.getByLabel('聊天室')).toHaveValue('-1002000002');
  await expect(page.getByLabel('包含已刪除')).toBeChecked();
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
