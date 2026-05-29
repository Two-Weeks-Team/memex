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

  // PR #12 REV-11 (CodeRabbit #15) — matchMedia alone is necessary but not
  // sufficient: the test must also prove that the CSS @media block actually
  // neutralised motion. We probe representative elements that have
  // transitions/animations in the unreduced state and assert their
  // computed style collapses to "0.01ms" (which is what the
  // `transition-duration: .01ms !important;` rule sets).
  const durations = await page.evaluate(() => {
    function dur(sel: string): { animation: string; transition: string } | null {
      const el = document.querySelector(sel);
      if (!el) return null;
      const cs = getComputedStyle(el as Element);
      return {
        animation: cs.animationDuration,
        transition: cs.transitionDuration,
      };
    }
    return {
      archTip: dur('.arch-tip'),
      qdGrid: dur('.qd-grid'),
      heroH1: dur('.hero h1'),
      bullet: dur('.qd-bullets[data-stagger] li'),
    };
  });
  // All four representative selectors should be present, and their durations
  // collapsed to the @media rule's "0.01ms" (or 0s if no animation was
  // specified at all). Anything in the 0.1s+ range means the @media block
  // failed to neutralise motion → user-visible regression.
  function shortEnough(d: { animation: string; transition: string } | null): boolean {
    if (!d) return false;
    const parse = (v: string) => {
      // CSS computed style values: comma-separated list of durations like "0.01ms, 0.01ms"
      return v.split(',').map(s => {
        const t = s.trim();
        if (t.endsWith('ms')) return parseFloat(t);
        if (t.endsWith('s')) return parseFloat(t) * 1000;
        return parseFloat(t);
      });
    };
    const maxMs = (v: string) => Math.max(...parse(v));
    return maxMs(d.animation) <= 1 && maxMs(d.transition) <= 1;
  }
  expect(shortEnough(durations.archTip),  '.arch-tip transition must collapse under reduced-motion').toBe(true);
  expect(shortEnough(durations.qdGrid),   '.qd-grid transition must collapse under reduced-motion').toBe(true);
  expect(shortEnough(durations.heroH1),   '.hero h1 transition must collapse under reduced-motion').toBe(true);
  expect(shortEnough(durations.bullet),   'qd-bullets[data-stagger] li transition must collapse under reduced-motion').toBe(true);
});
