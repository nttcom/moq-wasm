import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

export default defineConfig({
  base: '/moq-wasm/',
  plugins: [react()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'examples/call/src'),
      '@moqt': resolve(__dirname, 'lib/moqt')
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
      input: {
        index: resolve(__dirname, 'index.html'),
        message: resolve(__dirname, 'examples/message/index.html'),
        media: resolve(__dirname, 'examples/media/index.html'),
        publisher: resolve(__dirname, 'examples/media/publisher/index.html'),
        subscriber: resolve(__dirname, 'examples/media/subscriber/index.html'),
        call: resolve(__dirname, 'examples/call/index.html')
      }
    }
  }
})
