import { expect, test } from '@playwright/test'
import type { LivePlayerStats } from 'moq-wasm/media/player/livePlayer'
import { ClientTelemetry } from './clientTelemetry'

type Sent = { url: string; body: Record<string, unknown[]> & { clientId: string; page: string }; beacon: boolean }

function telemetry(): { telemetry: ClientTelemetry; sent: Sent[] } {
  const sent: Sent[] = []
  const telemetry = new ClientTelemetry(
    'live-viewer',
    () => 'http://observability/api/client-telemetry',
    (url, body, beacon) => sent.push({ url, body: JSON.parse(body), beacon })
  )
  return { telemetry, sent }
}

test('a flush sends the queued logs and stats once', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.log('warn', 'decoder reset', { track_name: 'video' })
  subject.sample({ bufferMs: 120 } as LivePlayerStats)

  // Act
  subject.flush()
  subject.flush()

  // Assert: the second flush has nothing left to send
  expect(sent).toHaveLength(1)
  expect(sent[0].url).toBe('http://observability/api/client-telemetry')
  expect(sent[0].body.clientId).toBe(subject.clientId)
  expect(sent[0].body.page).toBe('live-viewer')
  expect(sent[0].body.logs).toMatchObject([
    { level: 'warn', target: 'live-viewer', message: 'decoder reset', fields: { track_name: 'video' } }
  ])
  expect(sent[0].body.stats).toMatchObject([{ values: { bufferMs: 120 } }])
})

test('a report keeps only the newest 100 logs, the server limit', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  for (let index = 0; index < 101; index++) {
    subject.log('info', `log ${index}`)
  }

  // Act
  subject.flush(true)

  // Assert
  const logs = sent[0].body.logs as { message: string }[]
  expect(logs).toHaveLength(100)
  expect(logs[0].message).toBe('log 1')
  expect(sent[0].beacon).toBe(true)
})
