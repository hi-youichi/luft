import { test, expect } from '@playwright/test'
import { goto, expectNoCrash, waitForPageReady } from './helpers'

test.describe('Other Pages', () => {
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

  const pages = [
    { path: '/metrics', desc: 'Metrics' },
    { path: '/reports', desc: 'Reports' },
  ]

  for (const { path, desc } of pages) {
    test(`should render ${desc.toLowerCase()} page`, async ({ page }) => {
      await goto(page, path)
      await expectNoCrash(page)
      await waitForPageReady(page)

      // Verify the page rendered without crashing
      const heading = page.getByRole('heading').first()
      await expect(heading).toBeVisible({ timeout: 10_000 })
    })
  }

  test('should render live monitor page', async ({ page }) => {
    await goto(page, '/live')
    // Live Monitor uses persistent WebSocket — just verify no crash
    await expectNoCrash(page)
    await page.waitForTimeout(2000)
    await expectNoCrash(page)
  })
})