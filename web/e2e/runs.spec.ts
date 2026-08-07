import { test, expect } from '@playwright/test'
import { goto, expectNoCrash, waitForPageReady } from './helpers'

test.describe('Runs List', () => {
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

  test('should render runs page', async ({ page }) => {
    await goto(page, '/runs')
    await expectNoCrash(page)
    await waitForPageReady(page)

    // Verify the Runs heading is visible
    const heading = page.getByRole('heading', { name: 'Runs', exact: true })
    await expect(heading).toBeVisible({ timeout: 10_000 })
  })

  test('should navigate to run detail', async ({ page }) => {
    await goto(page, '/runs')
    await expectNoCrash(page)
    await waitForPageReady(page)

    // Check if any run link exists
    const runLink = page.locator('a[href^="/runs/"]').first()
    let linkVisible = false
    try {
      await runLink.waitFor({ state: 'visible', timeout: 5_000 })
      linkVisible = true
    } catch {
      linkVisible = false
    }

    test.skip(!linkVisible, 'No runs available to click')

    // Click the first run link and wait for navigation
    await Promise.all([
      page.waitForURL(/\/runs\//, { timeout: 10_000 }),
      runLink.click(),
    ])

    await expectNoCrash(page)
  })
})