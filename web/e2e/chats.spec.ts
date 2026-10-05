import type { Page } from '@playwright/test';
import { CHAT, expect, test } from './fixtures';

const rows = (page: Page) => page.locator('.chat-list li a');
const articles = (page: Page) => page.locator('article.msg');

test.describe('chat list', () => {
  test('lists every chat, with kinds, tracked dots and the error marker', async ({ page }) => {
    await page.goto('/chats');
    await expect(rows(page)).toHaveCount(7);
    await expect(page.locator('.chat-list a', { hasText: '舊專案群組' }).getByLabel('同步錯誤')).toBeVisible();
    await expect(page.locator('.chat-list a', { hasText: 'Empty Lab' }).getByLabel('未收集')).toBeVisible();
    await expect(page.locator('.chat-list a', { hasText: 'Rust 台灣社群' })).toContainText('258 則');
  });

  test('text filter, tabs, kind chips and sorting', async ({ page }) => {
    await page.goto('/chats');
    await expect(rows(page)).toHaveCount(7);
    await page.getByLabel('篩選對話').fill('rust');
    await expect(rows(page)).toHaveCount(1);
    await page.getByLabel('篩選對話').fill('不存在');
    await expect(page.getByText('沒有符合的對話')).toBeVisible();
    await page.getByLabel('篩選對話').fill('');

    await page.getByRole('tab', { name: '收集中' }).click();
    await expect(rows(page)).toHaveCount(5);
    await page.getByRole('tab', { name: '全部' }).click();

    const chip = page.getByRole('button', { name: '頻道', exact: true });
    await chip.click();
    await expect(chip).toHaveAttribute('aria-pressed', 'true');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first()).toContainText('Daily News');
    await chip.click();
    await page.getByRole('button', { name: '私訊', exact: true }).click();
    await expect(rows(page)).toHaveCount(2);
    await page.getByRole('button', { name: '私訊', exact: true }).click();

    await expect(rows(page).first()).toContainText('Rust 台灣社群');
    await page.getByLabel('排序').selectOption('title');
    await expect(rows(page).first()).toContainText('Alice');
    await page.getByLabel('排序').selectOption('message_count');
    await expect(rows(page).first()).toContainText('Rust 台灣社群');
    await expect(rows(page).last()).toContainText('0 則');
  });

  test('keyboard navigation moves focus and opens a chat', async ({ page }) => {
    await page.goto('/chats');
    await expect(rows(page)).toHaveCount(7);
    await rows(page).first().focus();
    await page.keyboard.press('Tab');
    await expect(rows(page).nth(1)).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/\/chats\/-?\d+$/);
    await expect(rows(page).nth(1)).toHaveAttribute('aria-current', 'page');
  });
});

test.describe('big chat timeline', () => {
  const url = `/chats/${CHAT.big}`;

  test('shows newest messages with date separators and loads older ones on scroll up', async ({ page }) => {
    await page.goto(url);
    await expect(articles(page)).toHaveCount(50);
    await expect(page.locator('#m-260')).toBeVisible();
    expect(await page.locator('.date-sep').count()).toBeGreaterThanOrEqual(2);
    await page.locator('.timeline').evaluate((el) => (el.scrollTop = 0));
    await expect(articles(page)).toHaveCount(100);
    // Keep scrolling to the very beginning.
    for (let i = 0; i < 6 && (await page.getByText('已到最早的訊息').count()) === 0; i++) {
      await page.locator('.timeline').evaluate((el) => (el.scrollTop = 0));
      await page.waitForTimeout(300);
    }
    await expect(page.getByText('已到最早的訊息')).toBeVisible();
    await expect(articles(page)).toHaveCount(258);
    await expect(page.locator('#m-1')).toHaveCount(1);
  });

  test('sender filter keeps only that sender', async ({ page }) => {
    await page.goto(url);
    await expect(articles(page)).toHaveCount(50);
    await page.getByLabel('寄件人').selectOption('2002');
    await expect(page).toHaveURL(/sender=2002/);
    await expect(articles(page).first()).toBeVisible();
    const names = await articles(page).locator('header strong').allTextContents();
    expect(names.length).toBeGreaterThan(0);
    expect(new Set(names)).toEqual(new Set(['Bob']));
  });

  test('jump to date shows messages up to that day', async ({ page }) => {
    await page.goto(url);
    await page.getByLabel('跳至日期').fill('2026-03-03');
    await expect(page).toHaveURL(/date=2026-03-03/);
    await expect(articles(page).last()).toContainText('第 90 則');
  });

  test('deleted messages are hidden until 顯示已刪除 is toggled', async ({ page }) => {
    await page.goto(`${url}?date=2026-03-03`);
    await expect(articles(page).last()).toContainText('第 90 則');
    await expect(page.getByText('ephemeral note')).toHaveCount(0);
    await page.getByLabel('顯示已刪除').click();
    await expect(page.getByLabel('顯示已刪除')).toBeChecked();
    await expect(page).toHaveURL(/deleted=1/);
    const deleted = page.locator('article.msg.deleted');
    await expect(deleted).toHaveCount(2);
    await expect(deleted.first().locator('.badge.red')).toContainText('已刪除');
    await expect(page.getByText('ephemeral note')).toBeVisible();
  });

  test('context view highlights the anchor and loads more before and after', async ({ page }) => {
    await page.goto(`${url}?message=120`);
    const anchor = page.locator('#m-120');
    await expect(anchor).toHaveClass(/anchor/);
    await expect(anchor).toHaveAttribute('aria-current', 'true');
    await expect(anchor).toBeInViewport();
    await expect(articles(page)).toHaveCount(51);
    await page.getByRole('button', { name: '載入更早的訊息' }).click();
    await expect(articles(page)).toHaveCount(101);
    await page.getByRole('button', { name: '載入較新的訊息' }).click();
    await expect(articles(page)).toHaveCount(151);
    await page.getByRole('button', { name: '離開脈絡檢視 ✕' }).click();
    await expect(page).not.toHaveURL(/message=/);
  });

  test('reply links jump to the replied message in context', async ({ page }) => {
    await page.goto(`${url}?message=101`);
    await page.locator('#m-101').getByRole('link', { name: /回覆 #100/ }).click();
    await expect(page).toHaveURL(/message=100/);
    const anchor = page.locator('#m-100');
    await expect(anchor).toHaveClass(/anchor/);
    await expect(anchor).toContainText('（已編輯）');
  });

  test('attachments of every kind render as chips', async ({ page }) => {
    await page.goto(`${url}?message=34`);
    await expect(page.locator('#m-30 .chip')).toHaveText('photo · 200 KB');
    await expect(page.locator('#m-31 .chip')).toHaveText(/video · demo\.mp4 · video\/mp4/);
    await expect(page.locator('#m-32 .chip')).toHaveText(/audio · song\.mp3/);
    await expect(page.locator('#m-33 .chip')).toHaveText(/voice/);
    await expect(page.locator('#m-34 .chip')).toHaveText('document · report.pdf · application/pdf · 1.5 MB');
    await expect(page.locator('#m-35 .chip')).toHaveText(/sticker/);
    await expect(page.locator('#m-36 .chip')).toHaveText(/animation · cat\.gif/);
    await expect(page.locator('#m-37 .chip')).toHaveText(/other/);
    await expect(page.locator('#m-38 .chip')).toHaveCount(2);
  });

  test('service message shows the placeholder instead of an empty body', async ({ page }) => {
    await page.goto(url);
    const service = page.locator('#m-259');
    await expect(service).toContainText('（系統訊息或無文字內容）');
    await expect(service.locator('.placeholder')).toHaveCSS('font-style', 'italic');
    await expect(page.locator('#m-258 .placeholder')).toHaveCount(0);
  });
});

test('service message with a known sender in another chat', async ({ page }) => {
  await page.goto(`/chats/${CHAT.family}`);
  const first = page.locator('#m-1');
  await expect(first).toContainText('（系統訊息或無文字內容）');
  await expect(first.locator('header strong')).toHaveText('Alice 愛麗絲');
});

test('messages from the chat itself fall back to the chat title', async ({ page }) => {
  await page.goto(`/chats/${CHAT.telegram}`);
  await expect(articles(page)).toHaveCount(2);
  expect(new Set(await articles(page).locator('header strong').allTextContents())).toEqual(new Set(['Telegram']));
  await page.goto(`/chats/${CHAT.news}`);
  await expect(articles(page)).toHaveCount(17);
  // Two legacy rows have no sender at all.
  expect(new Set(await articles(page).locator('header strong').allTextContents())).toEqual(new Set(['Daily News 每日快訊', '未知']));
});
