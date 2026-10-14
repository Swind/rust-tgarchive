import type { Page } from '@playwright/test';
import { CHAT, expect, test } from './fixtures';

// Exercise the real query/cache/DOM flow without requiring Telegram credentials.
async function mockEvents(page: Page) {
  await page.addInitScript(() => {
    const sources: EventTarget[] = [];
    class MockEventSource extends EventTarget {
      readonly url: string;
      readyState = 1;
      constructor(url: string) {
        super();
        this.url = url;
        sources.push(this);
      }
      close() {
        this.readyState = 2;
        const i = sources.indexOf(this);
        if (i >= 0) sources.splice(i, 1);
      }
    }
    Object.assign(window, {
      EventSource: MockEventSource,
      emitArchiveEvent: (type: string, data: unknown) => {
        for (const source of sources) source.dispatchEvent(new MessageEvent(type, { data: JSON.stringify(data) }));
      },
      archiveEventSourceCount: () => sources.length,
    });
  });
}

async function emit(page: Page, type = 'archive_changed') {
  await page.evaluate(({ type, chatId }) => {
    (window as unknown as { emitArchiveEvent: (type: string, data: unknown) => void })
      .emitArchiveEvent(type, type === 'resync' ? {} : { chat_ids: [chatId], global: false });
  }, { type, chatId: CHAT.big });
}

async function changedMessages(page: Page) {
  let changed = false;
  let fail = false;
  await page.route((url) => url.pathname === `/api/v1/chats/${CHAT.big}/messages`, async (route) => {
    if (fail) {
      await route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: 'SSE transient test' } } });
      return;
    }
    const response = await route.fetch();
    const body = await response.json();
    if (changed) {
      body.items = body.items.filter((m: { id: number }) => m.id !== 250 && m.id !== 70);
      for (const m of body.items) if (m.id === 251) m.text = 'SSE edited message';
      for (const m of body.items) if (m.id === 71) m.text = 'SSE edited context history';
      const url = new URL(route.request().url());
      if (!url.searchParams.has('before') && !url.searchParams.has('to')) {
        const newest = body.items.reduce((a: { id: number }, b: { id: number }) => a.id > b.id ? a : b);
        body.items.unshift({ ...newest, id: 261, text: 'SSE new message', timestamp: '2026-03-13T06:00:00Z' });
        body.items = body.items.slice(0, 50);
      }
    }
    await route.fulfill({ response, json: body });
  });
  return (shouldFail = false) => { changed = true; fail = shouldFail; };
}

test('SSE refreshes new, edited and deleted messages and follows the bottom', async ({ page }) => {
  await mockEvents(page);
  const change = await changedMessages(page);
  await page.goto(`/chats/${CHAT.big}`);
  await expect(page.locator('#m-260')).toBeInViewport();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { archiveEventSourceCount: () => number }).archiveEventSourceCount(),
  )).toBe(1);
  change();
  await emit(page);
  await expect(page.locator('#m-261')).toContainText('SSE new message');
  await expect(page.locator('#m-261')).toBeInViewport();
  await expect(page.locator('#m-251')).toContainText('SSE edited message');
  await expect(page.locator('#m-250')).toHaveCount(0);
});

test('SSE preserves the reader viewport while reading older messages', async ({ page }) => {
  await mockEvents(page);
  const change = await changedMessages(page);
  await page.goto(`/chats/${CHAT.big}`);
  await expect(page.locator('#m-260')).toBeInViewport();
  await page.locator('.timeline').evaluate((el) => { el.scrollTop = el.scrollHeight / 2; });
  const anchor = await page.locator('.timeline').evaluate((el) => {
    const top = el.getBoundingClientRect().top;
    const row = [...el.querySelectorAll('article.msg')].find((m) => m.getBoundingClientRect().bottom > top)!;
    return { id: row.id, offset: row.getBoundingClientRect().top - top };
  });
  change();
  await emit(page);
  await expect(page.locator('#m-261')).toHaveCount(1);
  await expect.poll(() => page.locator(`#${anchor.id}`).evaluate((el) =>
    el.getBoundingClientRect().top - el.closest('.timeline')!.getBoundingClientRect().top,
  )).toBeCloseTo(anchor.offset, 0);
  const action = page.getByRole('button', { name: /有新訊息/ });
  await expect(action).toBeVisible();
  await expect(action).toBeInViewport();
  await action.click();
  await expect(page.locator('#m-261')).toBeInViewport();
});

test('SSE reconnect resync refreshes current data without a reload', async ({ page }) => {
  await mockEvents(page);
  const change = await changedMessages(page);
  await page.goto(`/chats/${CHAT.big}`);
  await expect(page.locator('#m-260')).toBeInViewport();
  change();
  await emit(page, 'resync');
  await expect(page.locator('#m-261')).toContainText('SSE new message');
});

test('SSE refreshes manually loaded context history without jumping to latest', async ({ page }) => {
  await mockEvents(page);
  const change = await changedMessages(page);
  await page.goto(`/chats/${CHAT.big}?message=120`);
  await expect(page.locator('#m-120')).toBeInViewport();
  await page.getByRole('button', { name: '載入更早的訊息' }).click();
  await expect(page.locator('#m-70')).toHaveCount(1);
  change();
  await emit(page);
  await expect(page.locator('#m-71')).toContainText('SSE edited context history');
  await expect(page.locator('#m-70')).toHaveCount(0);
  await expect(page).toHaveURL(/message=120/);
  await expect(page.locator('#m-261')).toHaveCount(0);
});

test('a failed SSE-triggered refresh keeps existing messages and can recover', async ({ page }) => {
  await mockEvents(page);
  const change = await changedMessages(page);
  await page.goto(`/chats/${CHAT.big}`);
  await expect(page.locator('#m-260')).toBeInViewport();
  change(true);
  const failed = page.waitForResponse((response) => response.status() === 503 &&
    new URL(response.url()).pathname === `/api/v1/chats/${CHAT.big}/messages`);
  await emit(page);
  await failed;
  await expect(page.locator('#m-260')).toBeInViewport();
  change();
  await emit(page);
  await expect(page.locator('#m-261')).toBeInViewport();
});

test('SSE keeps the selected date and does not jump into live messages', async ({ page }) => {
  await mockEvents(page);
  const change = await changedMessages(page);
  await page.goto(`/chats/${CHAT.big}?date=2026-03-03`);
  await expect(page.locator('#m-90')).toBeInViewport();
  change();
  await emit(page);
  await expect(page.locator('#m-71')).toContainText('SSE edited context history');
  await expect(page).toHaveURL(/date=2026-03-03/);
  await expect(page.locator('#m-261')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /有新訊息/ })).toHaveCount(0);
});
