import { defineConfig } from 'vite'

export default defineConfig({
  base: '/moq-wasm/',
  build: {
    outDir: 'dist',
    rollupOptions: {
      input: {
        message: 'examples/message/index.html',
        publisher: 'examples/media/publisher/index.html',
        subscriber: 'examples/media/subscriber/index.html'
      }
    }
  }
})
