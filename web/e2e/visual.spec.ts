import { CHAT, expect, test } from './fixtures';

for (const scheme of ['light', 'dark'] as const) {
  test(`chats page (${scheme})`, async ({ page }) => {
    await page.addInitScript((s) => localStorage.setItem('tgarchive.theme', s), scheme);
    await page.goto(`/chats/${CHAT.big}`);
    await expect(page.locator('#m-260')).toBeVisible();
    await expect(page.locator('.chat-list a')).toHaveCount(7);
    await expect(page).toHaveScreenshot(`chats-${scheme}.png`);
  });
}

test('sync page', async ({ page }) => {
  await page.goto('/sync');
  await expect(page.locator('li.job')).toHaveCount(4);
  await page.locator('li.job[data-state="rate_limited"] button.job-head').click();
  await expect(page.locator('.chat-progress li')).toHaveCount(2);
  await expect(page).toHaveScreenshot('sync.png');
});
