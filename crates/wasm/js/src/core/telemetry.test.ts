import { expect, test } from '@playwright/test'
import type { LiveStats } from '../media/player/locLive'
import type { LogRecord, SpanRecord } from './init'
import { ClientTelemetry, type TelemetryOptions } from './telemetry'

type Sent = {
  url: string
  body: Record<string, unknown[]> & { token?: string; clientId: string; page: string }
  text: string
  beacon: boolean
}

function telemetry(
  status = 202,
  options: Partial<TelemetryOptions> = {}
): { telemetry: ClientTelemetry; sent: Sent[] } {
  const sent: Sent[] = []
  const telemetry = new ClientTelemetry(
    { url: 'http://observability/', page: 'live-viewer', ...options },
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

function log(message: string): LogRecord {
  return { timeMs: 1, level: 'info', target: 'moqt', message, fields: { track_name: 'video' } }
}

test('a flush sends the queued logs and spans once', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.log(log('SUBSCRIBE_OK received'), undefined)
  subject.span(span('moqt.subscriber.subscribe'), undefined)

  // Act
  subject.flush()
  subject.flush()

  // Assert: the second flush has nothing left to send
  expect(sent).toHaveLength(1)
  expect(sent[0].url).toBe('http://observability/api/client-telemetry')
  expect(sent[0].body.clientId).toBe(subject.clientId)
  expect(sent[0].body.page).toBe('live-viewer')
  expect(sent[0].body.logs).toEqual([log('SUBSCRIBE_OK received')])
  expect(sent[0].body.spans).toEqual([span('moqt.subscriber.subscribe')])
})

test('a flush sends the samples stats returns with the records of no session', () => {
  // Arrange
  const sample = {
    trackNamespace: 'anon/meeting/room/alice',
    mediaSource: 'camera',
    values: { bufferMs: 120 } as LiveStats
  }
  const { telemetry: subject, sent } = telemetry(202, { stats: () => [sample] })
  subject.span(span('moqt.session.connect'), 'jwt-a')

  // Act
  subject.flush()

  // Assert
  expect(sent.map(({ body }) => [body.token, body.stats])).toEqual([
    ['jwt-a', []],
    [undefined, [{ timeMs: expect.any(Number), ...sample }]]
  ])
})

test('a url function picks the server at each flush', () => {
  // Arrange
  let url = 'http://local-observability'
  const { telemetry: subject, sent } = telemetry(202, { url: () => url })
  subject.log(log('first'), undefined)
  subject.flush()
  url = 'http://cloud-observability/'
  subject.log(log('second'), undefined)

  // Act
  subject.flush()

  // Assert
  expect(sent.map(({ url }) => url)).toEqual([
    'http://local-observability/api/client-telemetry',
    'http://cloud-observability/api/client-telemetry'
  ])
})

test('a report keeps only the newest 100 logs, the server limit', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  for (let index = 0; index < 101; index++) {
    subject.log(log(`log ${index}`), undefined)
  }

  // Act
  subject.flush(true)

  // Assert
  const logs = sent[0].body.logs as LogRecord[]
  expect(logs).toHaveLength(100)
  expect(logs[0].message).toBe('log 1')
  expect(sent[0].beacon).toBe(true)
})

test("a session's token goes on its report and nowhere in its records", () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.span(span('moqt.session.connect'), 'jwt-a')
  subject.log(log('SUBSCRIBE_OK received'), 'jwt-a')

  // Act
  subject.flush()

  // Assert: the token appears once in the body, as the report's `token`
  expect(sent).toHaveLength(1)
  expect(sent[0].body.token).toBe('jwt-a')
  expect(sent[0].text.split('jwt-a')).toHaveLength(2)
})

test('the report of records without a token carries none', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.span(span('moqt.session.connect'), undefined)

  // Act
  subject.flush()

  // Assert
  expect(sent).toHaveLength(1)
  expect(sent[0].body).not.toHaveProperty('token')
})

test('sessions with different tokens are sent in separate reports', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry()
  subject.span(span('session a'), 'jwt-a')
  subject.span(span('session b'), 'jwt-b')
  subject.log(log('anonymous'), undefined)
  subject.log(log('session a'), 'jwt-a')

  // Act
  subject.flush()

  // Assert
  const reports = sent.map(({ body }) => ({
    token: body.token,
    spans: (body.spans as SpanRecord[]).map(({ name }) => name),
    logs: (body.logs as LogRecord[]).map(({ message }) => message)
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
  subject.span(span('first'), 'expired-jwt')
  subject.flush()
  await new Promise((resolve) => setTimeout(resolve))

  // Act
  subject.span(span('second'), 'expired-jwt')
  subject.flush()

  // Assert
  expect(sent.map(({ body }) => body.token)).toEqual(['expired-jwt', undefined])
})

test('redact rewrites the fields of the logs and spans sent', () => {
  // Arrange
  const { telemetry: subject, sent } = telemetry(202, {
    redact: ({ track_name: _, ...fields }) => ({ ...fields, outcome: 'redacted' })
  })
  subject.log(log('SUBSCRIBE_OK received'), undefined)
  subject.span(span('moqt.subscriber.subscribe'), undefined)

  // Act
  subject.flush()

  // Assert
  expect((sent[0].body.logs as LogRecord[])[0].fields).toEqual({ outcome: 'redacted' })
  expect((sent[0].body.spans as SpanRecord[])[0].fields).toEqual({ outcome: 'redacted' })
})
