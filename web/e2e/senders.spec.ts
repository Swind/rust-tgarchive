import { CHAT, expect, test } from './fixtures';

const XIAOMING = 4004;
const SELF = 9000;
const rows = (page: import('@playwright/test').Page) => page.locator('li.sender-row');

test.describe('senders list', () => {
  test('search by CJK substring, @username and ID; self badge', async ({ page }) => {
    await page.goto('/senders');
    await expect(rows(page).first()).toBeVisible();
    await page.getByLabel('搜尋使用者').fill('小明');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first()).toContainText('王小明');
    await expect(rows(page).first()).toContainText('4 則');
    await expect(rows(page).first()).toContainText('2 個聊天室');
    await expect(page).toHaveURL(/q=%E5%B0%8F%E6%98%8E/);
    await page.getByLabel('搜尋使用者').fill('@me_self');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('.badge.green')).toHaveText('自己');
    await page.getByLabel('搜尋使用者').fill(String(XIAOMING));
    await expect(rows(page)).toHaveCount(1);
    await page.getByLabel('搜尋使用者').fill('不存在的人');
    await expect(page.getByText('找不到符合的使用者')).toBeVisible();
  });

  test('sorting is kept in the URL', async ({ page }) => {
    await page.goto('/senders');
    const messages = page.getByRole('button', { name: '訊息數', exact: true });
    await expect(messages).toHaveAttribute('aria-pressed', 'true');
    await page.getByRole('button', { name: '名稱', exact: true }).click();
    await expect(page).toHaveURL(/sort=name/);
    await expect(rows(page).first()).toContainText('Alice');
    await page.getByRole('button', { name: '最近發言', exact: true }).click();
    await expect(page).toHaveURL(/sort=last_message/);
    await page.reload();
    await expect(page.getByRole('button', { name: '最近發言', exact: true })).toHaveAttribute('aria-pressed', 'true');
  });
});

test.describe('sender page', () => {
  test('breakdown, timeline, chat filter and in-sender search', async ({ page }) => {
    await page.goto(`/senders/${XIAOMING}`);
    await expect(page.getByRole('heading', { name: /王小明/ })).toBeVisible();
    await expect(page.getByText('4 則訊息')).toBeVisible();
    const chips = page.locator('.sender-chats button');
    await expect(chips).toHaveCount(2);
    const articles = page.locator('article.msg');
    await expect(articles).toHaveCount(4);
    await expect(articles.first().locator('.chat-title')).toBeVisible();
    await chips.filter({ hasText: '家庭群組' }).click();
    await expect(page).toHaveURL(/chat=-300001/);
    await expect(articles).toHaveCount(2);
    await chips.filter({ hasText: '家庭群組' }).click();
    await expect(articles).toHaveCount(4);

    await page.getByLabel('在此使用者的發言中搜尋').fill('小明的發言');
    await expect(page).toHaveURL(/q=/);
    await expect(page.locator('a.result')).toHaveCount(4);
    await expect(page.locator('a.result mark').first()).toBeVisible();
    await page.getByRole('button', { name: '時間', exact: true }).click();
    await expect(page).toHaveURL(/sort=time/);
    await expect(page.locator('a.result')).toHaveCount(4);
  });

  test('outgoing messages belong to the self account', async ({ page }) => {
    await page.goto(`/senders/${SELF}`);
    await expect(page.getByRole('heading', { name: /我自己/ })).toBeVisible();
    await expect(page.locator('h2 .badge.green')).toHaveText('自己');
    await expect(page.locator('article.msg')).toHaveCount(2);
    await expect(page.locator('article.msg').first()).toContainText('外送訊息');
  });

  test('unknown sender shows a not-found message', async ({ page }) => {
    await page.goto('/senders/123456');
    await expect(page.getByText('找不到使用者')).toBeVisible();
  });
});

test.describe('sender name navigation', () => {
  test('chat timeline, context view and search results link to the sender page', async ({ page }) => {
    await page.goto(`/chats/${CHAT.family}`);
    await page.locator('article.msg', { hasText: '小明的發言 101' }).locator('.sender-link').click();
    await expect(page).toHaveURL(new RegExp(`/senders/${XIAOMING}$`));

    await page.goto(`/chats/${CHAT.family}?message=101`);
    await page.locator('article.anchor .sender-link').click();
    await expect(page).toHaveURL(new RegExp(`/senders/${XIAOMING}$`));

    await page.goto('/search?q=小明的發言');
    await page.locator('a.result .sender-link').first().click();
    await expect(page).toHaveURL(new RegExp(`/senders/${XIAOMING}$`));
  });
});

test.describe('search page sender filter', () => {
  test('autocomplete selects a sender and the URL holds it', async ({ page }) => {
    await page.goto('/search?q=私訊');
    await expect(page.locator('a.result')).toHaveCount(8);
    await page.getByLabel('寄件人', { exact: true }).fill('Bob');
    await page.getByRole('option').filter({ hasText: 'Bob' }).getByRole('button').click();
    await expect(page).toHaveURL(/sender=2002/);
    await expect(page.locator('a.result')).toHaveCount(4);
    await expect(page.getByText('寄件人：Bob')).toBeVisible();
    await page.getByRole('button', { name: '清除寄件人' }).click();
    await expect(page).not.toHaveURL(/sender=/);
    await expect(page.locator('a.result')).toHaveCount(8);
  });

  test('deep link restores the filter', async ({ page }) => {
    await page.goto('/search?q=私訊&sender=2002');
    await expect(page.getByText('寄件人：Bob')).toBeVisible();
    await expect(page.locator('a.result')).toHaveCount(4);
  });
});

test.describe('post author and forward info', () => {
  test('channel signature and forward origin are rendered', async ({ page }) => {
    await page.goto(`/chats/${CHAT.news}`);
    await expect(page.locator('article.msg', { hasText: '署名快訊 13' }).locator('.post-author')).toHaveText('— 編輯小張');
    await expect(page.locator('article.msg', { hasText: '轉貼的快訊' })).toContainText('轉發自 原始來源電台');
    await expect(page.locator('article.msg', { hasText: '舊資料快訊 16' })).toContainText('未知');
    await page.goto(`/chats/${CHAT.family}`);
    await expect(page.locator('article.msg', { hasText: '轉傳的家庭訊息' })).toContainText('轉發自 Bob');
  });
});
