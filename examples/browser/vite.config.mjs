import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { resolve } from 'node:path'

export default defineConfig({
  base: '/moq-wasm/',
  plugins: [react()],
  resolve: {
    alias: {
      '@': resolve(import.meta.dirname, 'examples/meeting/src'),
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
      input: {
        index: resolve(import.meta.dirname, 'index.html'),
        message: resolve(import.meta.dirname, 'examples/message/index.html'),
        media: resolve(import.meta.dirname, 'examples/media/index.html'),
        publisher: resolve(import.meta.dirname, 'examples/media/publisher/index.html'),
        subscriber: resolve(import.meta.dirname, 'examples/media/subscriber/index.html'),
        'media-cmaf': resolve(import.meta.dirname, 'examples/media-cmaf/index.html'),
        'media-cmaf-publisher': resolve(import.meta.dirname, 'examples/media-cmaf/publisher/index.html'),
        'media-cmaf-subscriber': resolve(import.meta.dirname, 'examples/media-cmaf/subscriber/index.html'),
        onvif: resolve(import.meta.dirname, 'examples/onvif/index.html'),
        meeting: resolve(import.meta.dirname, 'examples/meeting/index.html'),
        webcodecs: resolve(import.meta.dirname, 'examples/webcodecs/index.html'),
        'remote-monitoring': resolve(import.meta.dirname, 'examples/remote-monitoring/index.html'),
        'live-viewer': resolve(import.meta.dirname, 'examples/live-viewer/index.html'),
        'moq-chat-moderation': resolve(import.meta.dirname, 'examples/moq-chat-moderation/index.html'),
        'moq-camera-detection': resolve(import.meta.dirname, 'examples/moq-camera-detection/index.html'),
        'moq-ptz-tracking': resolve(import.meta.dirname, 'examples/moq-ptz-tracking/index.html'),
        observability: resolve(import.meta.dirname, 'examples/observability/index.html')
      }
    }
  }
})
