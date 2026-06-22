import { X509Certificate, createHash } from 'node:crypto'
import { closeSync, existsSync, openSync, readFileSync, writeSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

export const MEDIA_PUBLISHER_PATH = '/moq-wasm/examples/media/publisher/index.html'
export const MEDIA_SUBSCRIBER_PATH = '/moq-wasm/examples/media/subscriber/index.html'
export const CALL_INDEX_PATH = '/moq-wasm/examples/call/index.html'

export function ensureLinuxEnvironment(): void {
  if (process.platform !== 'linux' && process.platform !== 'darwin') {
    throw new Error('The automated media E2E flow is supported on Linux and macOS only.')
  }
}

export function computeCertificateSpkiBase64(): string {
  const certPath = resolve(__dirname, '..', '..', 'relay', 'keys', 'cert.pem')
  const certificatePem = readFileSync(certPath, 'utf8')
  const certificate = new X509Certificate(certificatePem)
  const spkiDer = certificate.publicKey.export({ type: 'spki', format: 'der' })
  return createHash('sha256').update(spkiDer).digest('base64')
}

export function ensureFakeVideoCaptureFile(): string {
  const width = 160
  const height = 90
  const fps = 15
  const durationSeconds = 60
  const frameCount = fps * durationSeconds
  const fileName = 'moqt-call-e2e-fake-video-' + width + 'x' + height + '-' + fps + 'fps-' + durationSeconds + 's.y4m'
  const filePath = join(tmpdir(), fileName)
  if (existsSync(filePath)) {
    return filePath
  }

  const fd = openSync(filePath, 'w')
  try {
    writeSync(fd, 'YUV4MPEG2 W' + width + ' H' + height + ' F' + fps + ':1 Ip A1:1 C420jpeg\n')
    const ySize = width * height
    const chromaWidth = width / 2
    const chromaHeight = height / 2
    const uvSize = chromaWidth * chromaHeight
    const frame = Buffer.alloc(ySize + uvSize * 2)

    for (let frameIndex = 0; frameIndex < frameCount; frameIndex += 1) {
      for (let y = 0; y < height; y += 1) {
        for (let x = 0; x < width; x += 1) {
          frame[y * width + x] = (x * 2 + y * 3 + frameIndex * 5) % 256
        }
      }

      const uBase = ySize
      const vBase = ySize + uvSize
      for (let y = 0; y < chromaHeight; y += 1) {
        for (let x = 0; x < chromaWidth; x += 1) {
          const offset = y * chromaWidth + x
          frame[uBase + offset] = 96 + ((x + frameIndex) % 48)
          frame[vBase + offset] = 144 + ((y * 2 + frameIndex) % 48)
        }
      }

      writeSync(fd, 'FRAME\n')
      writeSync(fd, frame)
    }
  } finally {
    closeSync(fd)
  }

  return filePath
}
