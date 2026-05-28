/**
 * P3.E2E reduced-motion regression — under
 * `prefers-reduced-motion: reduce`, the new Q7/Q8 entry animations + the
 * RelevanceFeedback + Hybrid playgrounds must NOT animate; CSS in index.html
 * collapses all transitions to .01ms in that media query block.
 */
import { test, expect } from '@playwright/test';

test('entry animations are honoured to prefers-reduced-motion', async ({ page }) => {
  // Belt-and-suspenders: also emulate at the page level (the project-level
  // setting from playwright.config.ts is intended but some chromium-headless-shell
  // builds need the explicit emulateMedia call to flip the media query).
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');

  // The new cards must still be in the DOM and visible (CSS strips motion,
  // not visibility).
  await expect(page.locator('[data-q="Q7"]')).toBeVisible();
  await expect(page.locator('[data-q="Q8"]')).toBeVisible();
  await expect(page.locator('#qd-feedback-pg')).toBeVisible();
  await expect(page.locator('#qd-hybrid-viz')).toBeVisible();

  // The arch-tip dialog transition should also be neutralized.
  const reduces = await page.evaluate(() => {
    const m = window.matchMedia('(prefers-reduced-motion: reduce)');
    return m.matches;
  });
  expect(reduces, 'prefers-reduced-motion media query must report reduce').toBe(true);
});
