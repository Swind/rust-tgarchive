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

  test('mobile opens a conversation, exposes its tools and returns to the inbox', async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 760 });
    await page.goto('/chats');
    const inbox = page.getByRole('complementary', { name: '對話清單' });
    await inbox.getByRole('link', { name: /Rust 台灣社群/ }).click();
    await expect(page).toHaveURL(new RegExp(`/chats/${CHAT.big}$`));
    await expect(inbox).toBeHidden();
    const timeline = page.getByRole('region', { name: '訊息時間軸' });
    await expect(timeline).toBeVisible();
    const dimensions = await timeline.evaluate((el) => ({
      timelineHeight: el.clientHeight,
      overflow: document.querySelector('main')!.scrollWidth - document.querySelector('main')!.clientWidth,
    }));
    expect(dimensions.timelineHeight).toBeGreaterThan(240);
    expect(dimensions.overflow).toBeLessThanOrEqual(1);
    const tools = page.getByRole('button', { name: '對話工具' });
    await tools.click();
    await expect(page.getByLabel('寄件人')).toBeVisible();
    await expect(page.getByLabel('話題', { exact: true })).toBeVisible();
    await tools.click();
    await page.getByRole('button', { name: '圖片下載' }).click();
    await expect(page.getByLabel('自動下載封存版本')).toBeVisible();
    await page.getByRole('link', { name: '返回對話清單' }).click();
    await expect(page).toHaveURL(/\/chats$/);
    await expect(inbox.getByRole('link', { name: /Rust 台灣社群/ })).toBeVisible();
    await expect(timeline).toHaveCount(0);
  });
});

test.describe('forum topic collection', () => {
  const url = `/chats/${CHAT.big}`;
  const base = `**/api/v1${url}`;

  test('shows archived topics read-only without a collector', async ({ page }) => {
    await page.goto(url);
    await page.getByText('話題收集設定', { exact: true }).click();
    const settings = page.getByRole('group', { name: '要收集的話題' });
    await expect(settings.getByRole('checkbox', { name: '群友聊天' })).toBeDisabled();
    await expect(settings.getByRole('checkbox', { name: '每日班表' })).toBeDisabled();
    await expect(settings).toContainText('唯讀模式');
    await expect(settings).toContainText('包括新建立的話題');
    await expect(settings).toContainText('重新勾選會補收');
  });

  test('saves exclusions, keeps failed rechecks off, and discovers new topics on refresh', async ({ page }) => {
    const topics = [
      { id: 22, title: '群友聊天', collected: true },
      { id: 35, title: '每日班表', collected: true },
    ];
    await page.route('**/api/v1/sync/status', (route) =>
      route.fulfill({ json: { collector: { state: 'running' }, sync_jobs: [] } }));
    await page.route(`${base}/topics`, (route) => route.fulfill({ json: topics }));
    const writes: boolean[] = [];
    const settings = page.getByRole('group', { name: '要收集的話題' });
    const schedule = settings.getByRole('checkbox', { name: '每日班表' });
    await page.route(`${base}/topics/35`, async (route) => {
      expect(route.request().method()).toBe('PUT');
      const { collected } = route.request().postDataJSON();
      writes.push(collected);
      if (writes.length === 1) {
        await expect(schedule).toBeDisabled();
        await expect(settings.getByRole('status')).toContainText('正在儲存');
      }
      if (writes.length === 2) {
        return route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: '無法連線至 Telegram' } } });
      }
      topics[1]!.collected = collected;
      if (writes.length === 1) topics.push({ id: 41, title: '新話題', collected: true });
      await route.fulfill({ status: 204 });
    });
    await page.goto(url);
    await page.getByText('話題收集設定', { exact: true }).click();
    await expect(schedule).toBeChecked();
    await schedule.click();
    await expect(schedule).not.toBeChecked();
    await expect(settings.getByRole('checkbox', { name: '新話題' })).toBeChecked();
    await expect(settings.getByRole('checkbox', { name: '群友聊天' })).toBeChecked();
    await schedule.click();
    await expect(settings.getByRole('alert')).toContainText('無法連線至 Telegram');
    await expect(schedule).not.toBeChecked();
    await expect(schedule).toBeEnabled();
    await schedule.press('Space');
    await expect(schedule).toBeChecked();
    await expect(settings.getByRole('alert')).toHaveCount(0);
    expect(writes).toEqual([false, true, true]);
  });

  test('stopping chat collection disables cached topic settings', async ({ page }) => {
    let tracked = true;
    await page.route('**/api/v1/sync/status', (route) =>
      route.fulfill({ json: { collector: { state: 'running' }, sync_jobs: [] } }));
    await page.route(base, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: { ...await response.json(), tracked } });
    });
    await page.route(`${base}/tracking`, (route) => {
      expect(route.request().method()).toBe('DELETE');
      tracked = false;
      return route.fulfill({ json: { id: CHAT.big, kind: 'supergroup', tracked } });
    });
    await page.goto(url);
    await page.getByText('話題收集設定', { exact: true }).click();
    const checkbox = page.getByRole('checkbox', { name: '群友聊天' });
    await expect(checkbox).toBeEnabled();
    await page.getByRole('button', { name: '停止收集', exact: true }).click();
    await page.getByRole('button', { name: '確認停止收集' }).click();
    await expect(checkbox).toBeDisabled();
    await expect(page.getByText('請先開始收集此聊天室，才能變更話題設定。')).toBeVisible();
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

  test('topic badges and the topic dropdown filter the whole timeline', async ({ page }) => {
    await page.goto(url);
    const topics = page.getByLabel('話題', { exact: true });
    const badge = page.locator('#m-250').getByRole('link', { name: '依話題篩選：群友聊天' });
    await expect(badge).toHaveText('群友聊天');
    await expect(page.locator('#m-260')).toBeVisible();
    await expect(page.locator('#m-260').getByRole('link', { name: /^依話題篩選/ })).toHaveCount(0);

    await badge.click();
    await expect(page).toHaveURL(`${url}?topic_id=22`);
    await expect(topics).toHaveValue('22');
    await expect(articles(page)).toHaveCount(2);
    await expect(articles(page).first()).toHaveAttribute('id', 'm-50');
    await expect(articles(page).last()).toHaveAttribute('id', 'm-250');

    await topics.selectOption({ label: '所有話題' });
    await expect(page).toHaveURL(url);
    await expect(page.locator('#m-260')).toBeVisible();
    await topics.selectOption({ label: '每日班表' });
    await expect(page).toHaveURL(`${url}?topic_id=35`);
    await page.reload();
    await expect(topics).toHaveValue('35');
    await expect(articles(page)).toHaveCount(1);
    await expect(articles(page).first()).toHaveAttribute('id', 'm-150');
  });

  test('topic filters leave anchor context intact until the reader opens a timeline', async ({ page }) => {
    const context = `${url}?message=250&topic_id=35`;
    await page.goto(context);
    const anchor = page.locator('#m-250');
    await expect(anchor).toHaveAttribute('aria-current', 'true');
    await expect(page.locator('#m-260')).toHaveCount(1);

    await anchor.getByRole('link', { name: '依話題篩選：群友聊天' }).click();
    await expect(page).toHaveURL(`${url}?topic_id=22`);
    await expect(articles(page)).toHaveCount(2);

    await page.goBack();
    await expect(page).toHaveURL(context);
    await expect(anchor).toHaveAttribute('aria-current', 'true');
    await page.getByRole('button', { name: '離開脈絡檢視 ✕' }).click();
    await expect(page).toHaveURL(`${url}?topic_id=35`);
    await expect(articles(page)).toHaveCount(1);
    await expect(articles(page).first()).toHaveAttribute('id', 'm-150');
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
    await expect(deleted.first().getByText(/^已刪除/)).toBeVisible();
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

test.describe('page scrolling', () => {
  for (const viewport of [
    { width: 1300, height: 850 },
    { width: 390, height: 800 },
  ]) {
    test(`only the timeline scrolls at ${viewport.width}px`, async ({ page }) => {
      await page.setViewportSize(viewport);
      await page.goto(`/chats/${CHAT.big}`);
      await expect(page.locator('article.msg').first()).toBeVisible();
      // Load older pages too: the bug only showed once many messages were rendered.
      for (let i = 0; i < 3; i++) {
        await page.locator('.timeline').evaluate((el) => (el.scrollTop = 0));
        await page.waitForTimeout(400);
      }
      const sizes = await page.evaluate(() => {
        const doc = document.scrollingElement!;
        const main = document.querySelector('main')!;
        const timeline = document.querySelector('.timeline')!;
        return {
          doc: doc.scrollHeight - doc.clientHeight,
          main: main.scrollHeight - main.clientHeight,
          timeline: timeline.scrollHeight - timeline.clientHeight,
        };
      });
      expect(sizes.doc, 'document must not scroll').toBeLessThanOrEqual(1);
      expect(sizes.main, 'main must not scroll on the chat page').toBeLessThanOrEqual(1);
      expect(sizes.timeline, 'timeline scrolls').toBeGreaterThan(100);
    });
  }
});
