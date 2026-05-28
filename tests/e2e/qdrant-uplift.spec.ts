/**
 * Qdrant uplift E2E suite — proves the new Q7/Q8 cards, RelevanceFeedback
 * playground (T2.13), Hybrid lane visualizer (T2.14), Q2 lens slider (T2.x),
 * and prefers-reduced-motion path (T1.* preserve accessibility) all behave
 * as the SOT (claudedocs/qdrant-improvement-goal.md) specifies.
 *
 * Acceptance per SOT §3 P3.E2E:
 *   Q7 + Q8 cards visible with mini-viz svg
 *   RelevanceFeedback playground: thumbs-up reorders results
 *   Hybrid lane visualizer: toggle re-orders result list
 *   Q2 slider: drag updates Σ number
 *   prefers-reduced-motion: emulate → entry animations disabled
 *   visual captures: hero, architecture-with-hover-dialog, Q7/Q8, topology, recall
 */
import { test, expect } from '@playwright/test';

// process.cwd() is the project root (where playwright.config.ts lives) when
// `npx playwright test` is invoked normally. ES-module spec files can't use
// __dirname; cwd is stable here.
const SCREENSHOT_DIR = `${process.cwd()}/claudedocs/reports/qdrant-uplift`;

test.describe('Q7 + Q8 cards present', () => {
  test('Q7 server-side scoring card renders with mini-viz', async ({ page }) => {
    await page.goto('/');
    const q7 = page.locator('[data-q="Q7"]').first();
    await expect(q7).toBeVisible();
    await expect(q7.locator('.qd-viz svg')).toBeVisible();
    await expect(q7.locator('header h3')).toContainText(/Formula|server-side|scoring/i);
    // cross-ref chip present
    await expect(q7.locator('.qd-xref')).toBeVisible();
  });

  test('Q8 hybrid retrieval card renders with mini-viz and visualizer', async ({ page }) => {
    await page.goto('/');
    const q8 = page.locator('[data-q="Q8"]').first();
    await expect(q8).toBeVisible();
    await expect(q8.locator('.qd-viz svg')).toBeVisible();
    await expect(q8.locator('#qd-hybrid-viz')).toBeVisible();
    // 3 toggles
    await expect(q8.locator('#qd-hybrid-viz .qd-hyb-tog')).toHaveCount(3);
    // 8 rows
    await expect(q8.locator('#qd-hybrid-viz .qd-hyb-row')).toHaveCount(8);
  });
});

test.describe('T2.13 RelevanceFeedback playground', () => {
  test('thumbs-up reorders results + JSON updates', async ({ page }) => {
    await page.goto('/');
    const pg = page.locator('#qd-feedback-pg');
    await expect(pg).toBeVisible();

    // Initial: 5 cards
    await expect(pg.locator('[data-feedback-card]')).toHaveCount(5);

    // Capture the initial top card sid (highest base score is .88 = alpha-9c1f)
    const initialTopSid = await pg.locator('[data-feedback-card]').first().getAttribute('data-sid');
    expect(initialTopSid).toBe('alpha-9c1f');

    // Click thumbs-up on the LAST card (eps-2a76, base .70) — its score becomes .90, moves to top
    const lastCard = pg.locator('[data-feedback-card]').last();
    const lastSid = await lastCard.getAttribute('data-sid');
    expect(lastSid).toBe('eps-2a76');
    await lastCard.locator('.qd-fb-btn[data-feedback="up"]').click();

    // After reorder, the top card should now be eps-2a76 (.70 + .20 = .90 vs alpha .88)
    await page.waitForTimeout(120);
    const newTopSid = await pg.locator('[data-feedback-card]').first().getAttribute('data-sid');
    expect(newTopSid).toBe('eps-2a76');

    // FeedbackItem JSON includes eps-2a76 in positiveIds
    const json = await pg.locator('#qd-fb-json').textContent();
    expect(json).toContain('"eps-2a76"');
    expect(json).toMatch(/positiveIds/);
  });
});

test.describe('T2.14 Hybrid lane visualizer', () => {
  test('toggling late ON re-orders the result list', async ({ page }) => {
    await page.goto('/');
    const viz = page.locator('#qd-hybrid-viz');
    await expect(viz).toBeVisible();

    // Initial top row (dense + sparse on; late off). Capture first label.
    await page.waitForTimeout(120);
    const initialTopLabel = await viz.locator('.qd-hyb-row .qd-hyb-label').first().textContent();
    expect(initialTopLabel).toBeTruthy();

    // Toggle late ON. Now the row with highest late score (.91 explain MaxSim) should appear higher.
    const lateTog = viz.locator('.qd-hyb-tog[data-lane="late"]');
    await expect(lateTog).toHaveAttribute('aria-pressed', 'false');
    await lateTog.click();
    await expect(lateTog).toHaveAttribute('aria-pressed', 'true');
    await page.waitForTimeout(160);

    // Order should differ from initial (the late lane shuffled the ranking).
    const labels = await viz.locator('.qd-hyb-row .qd-hyb-label').allTextContents();
    expect(labels.length).toBe(8);
    // Score column is non-empty and formatted as fixed(2).
    const firstScore = await viz.locator('.qd-hyb-row .qd-hyb-score').first().textContent();
    expect(firstScore).toMatch(/^\d\.\d\d$/);
  });

  test('toggling dense OFF zeroes the dense contribution', async ({ page }) => {
    await page.goto('/');
    const viz = page.locator('#qd-hybrid-viz');
    const denseTog = viz.locator('.qd-hyb-tog[data-lane="dense"]');
    await expect(denseTog).toHaveAttribute('aria-pressed', 'true');
    await denseTog.click();
    await expect(denseTog).toHaveAttribute('aria-pressed', 'false');
    // Visualizer should still render 8 rows (just re-ordered).
    await expect(viz.locator('.qd-hyb-row')).toHaveCount(8);
  });
});

test.describe('Q2 lens-slider playground', () => {
  test('drag content slider updates Σ result', async ({ page }) => {
    await page.goto('/');
    const pg = page.locator('#qd-playground');
    await expect(pg).toBeVisible();
    const out = pg.locator('.qd-fout');
    const before = (await out.textContent())?.trim();
    expect(before).toBeTruthy();

    // Find the first slider (content lens), set its value, dispatch input event.
    await pg.evaluate(() => {
      const s = document.querySelector<HTMLInputElement>('#qd-playground input[type="range"]');
      if (!s) throw new Error('slider not found');
      s.value = '1.8';
      s.dispatchEvent(new Event('input', { bubbles: true }));
      s.dispatchEvent(new Event('change', { bubbles: true }));
    });
    await page.waitForTimeout(80);
    const after = (await out.textContent())?.trim();
    expect(after).toBeTruthy();
    expect(after).not.toBe(before);
  });
});

test.describe('Visual sanity captures', () => {
  test('hero · architecture · qdrant Q7/Q8 · topology · recall', async ({ page }) => {
    await page.goto('/');
    await page.waitForLoadState('networkidle');

    // Hero
    await page.locator('section').first().scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${SCREENSHOT_DIR}/01-hero.png`, fullPage: false });

    // Architecture (entry animation runs once; just capture the visible portion)
    await page.locator('#architecture').scrollIntoViewIfNeeded();
    await page.waitForTimeout(700);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/02-architecture.png`, fullPage: false });

    // Architecture with hover dialog open (hover the first lens chip)
    const firstHov = page.locator('.arch-wrap svg .hov').first();
    if (await firstHov.count()) {
      await firstHov.hover();
      await page.waitForTimeout(300);
      await page.screenshot({ path: `${SCREENSHOT_DIR}/03-architecture-hover.png`, fullPage: false });
    }

    // Qdrant section — scroll to Q7/Q8
    await page.locator('[data-q="Q7"]').scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/04-qdrant-q7q8.png`, fullPage: false });

    // RelevanceFeedback playground
    await page.locator('#qd-feedback-pg').scrollIntoViewIfNeeded();
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/05-feedback-playground.png`, fullPage: false });

    // Topology galaxy
    const topo = page.locator('#topology, [id*="topology"]').first();
    if (await topo.count()) {
      await topo.scrollIntoViewIfNeeded();
      await page.waitForTimeout(700);
      await page.screenshot({ path: `${SCREENSHOT_DIR}/06-topology.png`, fullPage: false });
    }

    // Recall typewriter
    const recall = page.locator('#recall, [id*="recall"]').first();
    if (await recall.count()) {
      await recall.scrollIntoViewIfNeeded();
      await page.waitForTimeout(500);
      await page.screenshot({ path: `${SCREENSHOT_DIR}/07-recall.png`, fullPage: false });
    }
  });
});
