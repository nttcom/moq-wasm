import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { globSync } from 'node:fs'
import { resolve } from 'node:path'

export default defineConfig({
  base: '/moq-wasm/',
  plugins: [react()],
  resolve: {
    alias: {
      '@moqt': resolve(import.meta.dirname, 'lib/moqt'),
      '@player': resolve(import.meta.dirname, 'lib/player')
    }
  },
  server: {
    fs: {
      allow: ['..', '../..']
    }
  },
  build: {
    outDir: 'dist',
    rollupOptions: {
      input: globSync('{index.html,examples/**/index.html}', { cwd: import.meta.dirname }).map((page) =>
        resolve(import.meta.dirname, page)
      )
    }
  }
})
