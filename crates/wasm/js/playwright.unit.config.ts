import { readFileSync } from 'node:fs'
import { registerHooks } from 'node:module'
import { fileURLToPath } from 'node:url'
import { defineConfig } from '@playwright/test'

// Emulates Vite's `?raw` import for the unit tests, which run in Node without Vite.
registerHooks({
  load(url, context, nextLoad) {
    if (!url.endsWith('?raw')) {
      return nextLoad(url, context)
    }
    const text = readFileSync(fileURLToPath(url), 'utf8')
    return { format: 'module', source: `export default ${JSON.stringify(text)}`, shortCircuit: true }
  }
})

export default defineConfig({
  testDir: 'src',
  testMatch: /\.test\.ts$/,
  outputDir: './test-results/unit',
  reporter: 'list'
})
