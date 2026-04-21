import { X509Certificate, createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

export const MEDIA_PUBLISHER_PATH = '/moq-wasm/examples/media/publisher/index.html'
export const MEDIA_SUBSCRIBER_PATH = '/moq-wasm/examples/media/subscriber/index.html'

export function ensureLinuxEnvironment(): void {
  if (process.platform !== 'linux') {
    throw new Error('The automated media E2E flow is supported on Linux only.')
  }
}

export function computeCertificateSpkiBase64(): string {
  const certPath = resolve(__dirname, '..', 'moqt-server-sample', 'keys', 'cert.pem')
  const certificatePem = readFileSync(certPath, 'utf8')
  const certificate = new X509Certificate(certificatePem)
  const spkiDer = certificate.publicKey.export({ type: 'spki', format: 'der' })
  return createHash('sha256').update(spkiDer).digest('base64')
}
