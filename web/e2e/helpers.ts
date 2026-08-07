import { type Page, expect } from '@playwright/test'

/**
 * Navigate to a path and wait for the page to load (Vite SPA: load event, not networkidle,
 * because HMR WebSocket keeps the connection alive indefinitely).
 * Uses a short timeout to avoid hanging on slow pages.
 */
export async function goto(page: Page, path: string) {
  await page.goto(path, { waitUntil: 'load', timeout: 15000 })
}

/**
 * Assert the page is not showing an error boundary.
 * The route-error-boundary renders "页面加载出错" as its heading.
 */
export async function expectNoCrash(page: Page) {
  await expect(page.getByRole('heading', { name: '页面加载出错' })).toHaveCount(0)
}

/**
 * Wait for the page to be fully rendered — waits for a heading to appear.
 * Falls back gracefully if the page has no heading or is still loading.
 */
export async function waitForPageReady(page: Page) {
  // Wait for any heading to appear (indicates the lazy-loaded component has rendered)
  try {
    await page.waitForSelector('h1, h2, h3', { timeout: 8000 }).catch(() => {})
    await page.waitForTimeout(300)
  } catch {
    // Page was closed or test timed out — continue
  }
}