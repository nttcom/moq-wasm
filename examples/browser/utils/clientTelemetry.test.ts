import { expect, test } from '@playwright/test'
import type { LiveStats } from 'moq-wasm/media'
import type { SpanRecord } from 'moq-wasm'
import { ClientTelemetry } from './clientTelemetry'

type Sent = {
  url: string
  body: Record<string, unknown[]> & { token?: string; clientId: string; page: string }
  text: string
  beacon: boolean
}

function telemetry(status = 202): { telemetry: ClientTelemetry; sent: Sent[] } {
  const sent: Sent[] = []
  const telemetry = new ClientTelemetry(
    'live-viewer',
    () => 'http://observability/api/client-telemetry',
    async (url, text, beacon) => {
      sent.push({ url, body: JSON.parse(text), text, beacon })
      return status
    }
  )
  return { telemetry, sent }
}

function span(name: string): SpanRecord {
  return {
    traceId: '0123456789abcdef0123456789abcdef',
    spanId: '0123456789abcdef',
    name,
    startTimeMs: 1,
    endTimeMs: 2,
    fields: { outcome: 'ok' }
  }
}

function log(message: string): Parameters<ClientTelemetry['wasmLog']>[0] {
  return { timeMs: 1, level: 'info', target: 'moqt', message, fields: { track_name: 'video' } }
}

test('a flush sends the queued logs and stats once', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.log('warn', 'decoder reset', { track_name: 'video' })
  subject.sample({
    trackNamespace: 'anon/meeting/room/alice',
    mediaSource: 'camera',
    values: { bufferMs: 120 } as LiveStats
  })

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
  expect(sent[0].body.stats).toMatchObject([
    { trackNamespace: 'anon/meeting/room/alice', mediaSource: 'camera', values: { bufferMs: 120 } }
  ])
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

test('a flush sends the wasm spans alone', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  const subscribe = span('moqt.subscriber.subscribe')
  subject.wasmSpan(subscribe, undefined)

  // Act
  subject.flush()

  // Assert
  expect(sent).toHaveLength(1)
  expect(sent[0].body.spans).toEqual([subscribe])
  expect(sent[0].body.logs).toEqual([])
})

test("a session's token goes on its report and nowhere in its records", () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.wasmSpan(span('moqt.session.connect'), 'jwt-a')
  subject.wasmLog(log('SUBSCRIBE_OK received'), 'jwt-a')

  // Act
  subject.flush()

  // Assert: the token appears once in the body, as the report's `token`
  expect(sent).toHaveLength(1)
  expect(sent[0].body.token).toBe('jwt-a')
  expect(sent[0].text.split('jwt-a')).toHaveLength(2)
  expect(sent[0].body.spans).toEqual([span('moqt.session.connect')])
})

test('the report of records without a token carries none', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.wasmSpan(span('moqt.session.connect'), undefined)
  subject.log('info', 'page opened')

  // Act
  subject.flush()

  // Assert
  expect(sent).toHaveLength(1)
  expect(sent[0].body).not.toHaveProperty('token')
  expect(sent[0].body.spans).toHaveLength(1)
  expect(sent[0].body.logs).toHaveLength(1)
})

test('sessions with different tokens are sent in separate reports', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.wasmSpan(span('session a'), 'jwt-a')
  subject.wasmSpan(span('session b'), 'jwt-b')
  subject.wasmLog(log('anonymous'), undefined)
  subject.wasmLog(log('session a'), 'jwt-a')

  // Act
  subject.flush()

  // Assert
  const reports = sent.map(({ body }) => ({
    token: body.token,
    spans: (body.spans as SpanRecord[]).map(({ name }) => name),
    logs: (body.logs as { message: string }[]).map(({ message }) => message)
  }))
  expect(reports).toEqual([
    { token: 'jwt-a', spans: ['session a'], logs: ['session a'] },
    { token: 'jwt-b', spans: ['session b'], logs: [] },
    { token: undefined, spans: [], logs: ['anonymous'] }
  ])
})

test('the records of a token the server rejected are sent without it', async () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry(401)
  subject.wasmSpan(span('first'), 'expired-jwt')
  subject.flush()
  await new Promise((resolve) => setTimeout(resolve))

  // Act
  subject.wasmSpan(span('second'), 'expired-jwt')
  subject.flush()

  // Assert
  expect(sent.map(({ body }) => body.token)).toEqual(['expired-jwt', undefined])
})
