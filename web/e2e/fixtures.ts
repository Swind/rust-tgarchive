import { test as base, expect } from '@playwright/test';

export { expect };

/** Ids of the fixture written by `cargo run --example seed_fixture`. */
export const CHAT = {
  alice: 1001,
  telegram: 777000,
  big: -1002000001,
  news: -1002000002,
  empty: -1002000003,
  family: -300001,
  old: -300002,
} as const;

/**
 * Every test gets a frozen clock (stable relative times) and may not reach any host other than
 * the app server: such requests are aborted and fail the test.
 */
export const test = base.extend({
  page: async ({ page, baseURL }, use) => {
    const host = new URL(baseURL ?? 'http://127.0.0.1').host;
    const external: string[] = [];
    await page.route(
      (url) => /^https?:$/.test(url.protocol) && url.host !== host,
      (route) => {
        external.push(route.request().url());
        return route.abort();
      },
    );
    await page.clock.setFixedTime(new Date('2026-03-12T06:00:00Z'));
    await use(page);
    expect(external, 'requests to external hosts').toEqual([]);
  },
});
