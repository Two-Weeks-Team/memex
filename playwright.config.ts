import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright config for the Qdrant uplift E2E suite (qdrant-improvement-goal.md
 * §3 P3.E2E). The suite asserts the new Q7/Q8 cards, RelevanceFeedback
 * playground, Hybrid lane visualizer, Q2 slider interactivity, and the
 * prefers-reduced-motion accessibility path.
 *
 * Static index.html is served by a python3 -m http.server launched as the
 * web server. The artifacts (screenshots, traces) land under
 * claudedocs/reports/qdrant-uplift/ for the engine PR body to link.
 */
export default defineConfig({
  testDir: './tests/e2e',
  fullyParallel: false,        // single static server, sequential assertions
  forbidOnly: true,
  retries: 0,
  workers: 1,
  reporter: [
    ['list'],
    ['json', { outputFile: 'claudedocs/reports/qdrant-uplift/results.json' }],
  ],
  outputDir: 'claudedocs/reports/qdrant-uplift/artifacts',
  use: {
    baseURL: 'http://localhost:4173',
    headless: true,
    viewport: { width: 1440, height: 900 },
    screenshot: 'only-on-failure',
    // PR #12 REV-6 (CodeRabbit #8) — `on-first-retry` never fires because
    // `retries: 0`. Switched to `retain-on-failure` so a failing test always
    // leaves a trace; passing tests have no trace overhead.
    trace: 'retain-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium-default',
      use: { ...devices['Desktop Chrome'] },
      // reduced-motion.spec.ts MUST only run under the reduced-motion project.
      testIgnore: /reduced-motion\.spec\.ts/,
    },
    {
      name: 'chromium-reduced-motion',
      use: { ...devices['Desktop Chrome'], reducedMotion: 'reduce' },
      testMatch: /reduced-motion\.spec\.ts/,
    },
  ],
  webServer: {
    command: 'python3 -m http.server 4173',
    port: 4173,
    timeout: 15_000,
    reuseExistingServer: !process.env.CI,
  },
});
