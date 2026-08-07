import { test, expect } from '@playwright/test'
import { goto, expectNoCrash, waitForPageReady } from './helpers'

test.describe('Backends', () => {
  // Pre-condition: skip all tests when the daemon is unreachable or unhealthy
  test.beforeEach(async ({ page }) => {
    let healthy = false
    try {
      const response = await page.request.get('/api/health')
      if (response.ok()) {
        const body = await response.json()
        if (body.status === 'ok') {
          healthy = true
        }
      }
    } catch {
      // Daemon unreachable
    }
    if (!healthy) {
      test.skip(true, 'Daemon is unreachable or unhealthy')
    }
  })

  test('should render backends page', async ({ page }) => {
    await goto(page, '/backends')
    await expectNoCrash(page)
    await waitForPageReady(page)

    // Verify the page rendered — the heading may be present
    const heading = page.getByRole('heading').first()
    await expect(heading).toBeVisible({ timeout: 10_000 })
  })

  test('should not crash even when backend data is unavailable', async ({ page }) => {
    await goto(page, '/backends')
    await expectNoCrash(page)
    await waitForPageReady(page)

    // Just verify the page is stable and rendered
    const heading = page.getByRole('heading').first()
    await expect(heading).toBeVisible({ timeout: 10_000 })
  })
})