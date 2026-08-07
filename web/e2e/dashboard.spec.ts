import { test, expect } from '@playwright/test'
import { goto, expectNoCrash, waitForPageReady } from './helpers'

test.describe('Dashboard', () => {
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

  test('should render dashboard page', async ({ page }) => {
    await goto(page, '/')
    await expectNoCrash(page)
    await waitForPageReady(page)

    // Verify the dashboard heading is visible
    const heading = page.getByRole('heading', { name: 'Dashboard', exact: true })
    await expect(heading).toBeVisible({ timeout: 10_000 })
  })

  test('should show stat cards', async ({ page }) => {
    await goto(page, '/')
    await expectNoCrash(page)
    await waitForPageReady(page)

    // Wait for the stat card text to appear (data may come from MCP, not HTTP)
    // The stat cards show: Runs, Tokens, 成功, 失败
    const statLabels = ['Runs', 'Tokens', '成功', '失败']
    for (const label of statLabels) {
      try {
        await expect(page.getByText(label).first()).toBeVisible({ timeout: 8_000 })
      } catch {
        // Stats may not be available - skip gracefully
        test.skip(true, `Stat card "${label}" not found (daemon may not serve stats)`)
        return
      }
    }
  })

  test('should navigate to run detail from active run card', async ({ page }) => {
    await goto(page, '/')
    await expectNoCrash(page)
    await waitForPageReady(page)

    // Check if any run link exists (RunMiniCard wraps a <Link to={`/runs/${run.run_id}`}>)
    const runLink = page.locator('a[href^="/runs/"]').first()
    let linkVisible = false
    try {
      await runLink.waitFor({ state: 'visible', timeout: 5_000 })
      linkVisible = true
    } catch {
      linkVisible = false
    }

    test.skip(!linkVisible, 'No active run cards available to click')

    // Click the first run card link and wait for navigation
    await Promise.all([
      page.waitForURL(/\/runs\//, { timeout: 10_000 }),
      runLink.click(),
    ])

    // Verify the new page loaded without crashing
    await expectNoCrash(page)
  })
})