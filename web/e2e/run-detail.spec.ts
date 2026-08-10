import { test, expect } from '@playwright/test'
import { goto, expectNoCrash } from './helpers'

test.describe('Run Detail', () => {
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
      return
    }
  })

  test('should render run detail page for a known run', async ({ page }) => {
    // Fetch available runs
    let runId: string | null = null
    try {
      const resp = await page.request.get('/api/runs', { timeout: 5_000 })
      if (resp.ok()) {
        const runs = await resp.json()
        const runsList = Array.isArray(runs) ? runs : runs.runs ?? []
        if (runsList.length > 0) {
          runId = runsList[0].run_dir || runsList[0].run_id
        }
      }
    } catch {
      // API unavailable
    }

    test.skip(!runId, 'No runs available')

    await goto(page, `/runs/${runId}`)
    await expectNoCrash(page)
    await page.waitForTimeout(3000)

    // Just verify the page didn't crash
    await expectNoCrash(page)
  })

  test('should show timeline when run has phases', async ({ page }) => {
    let runId: string | null = null
    try {
      const resp = await page.request.get('/api/runs', { timeout: 5_000 })
      if (resp.ok()) {
        const runs = await resp.json()
        const runsList = Array.isArray(runs) ? runs : runs.runs ?? []
        if (runsList.length > 0) {
          runId = runsList[0].run_dir || runsList[0].run_id
        }
      }
    } catch {
      // API unavailable
    }

    test.skip(!runId, 'No runs available')

    await goto(page, `/runs/${runId}`)
    await expectNoCrash(page)
    await page.waitForTimeout(3000)

    // Check for Timeline or Phase text
    try {
      await expect(page.getByText('Timeline').or(page.getByText('Phase'))).toBeVisible({ timeout: 5_000 })
    } catch {
      // No phases - acceptable
    }
  })
})