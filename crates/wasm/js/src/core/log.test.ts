import { expect, test } from '@playwright/test'
import type { LogRecord } from './init'
import { type WorkerLogMessage, bindLogToken, forwardSdkLog, receiveWorkerLog, sdkLog, setSdkLogSink } from './log'

function sinkAt(level: LogRecord['level']): LogRecord[] {
  const records: LogRecord[] = []
  setSdkLogSink((record) => records.push(record), level)
  return records
}

function tokenSinkAt(level: LogRecord['level']): (string | undefined)[] {
  const tokens: (string | undefined)[] = []
  setSdkLogSink((_record, token) => tokens.push(token), level)
  return tokens
}

test('a log below the sink level is not handed to it', () => {
  // Arrange
  const records = sinkAt('warn')

  // Act
  forwardSdkLog(undefined, 'info', 'moq-wasm::media::live_player', 'adaptive quality changed')
  forwardSdkLog(undefined, 'error', 'moq-wasm::media::live_player', 'catalog failed')

  // Assert
  expect(records.map(({ level, message }) => [level, message])).toEqual([['error', 'catalog failed']])
})

test('fields that are not a string, number or boolean are flattened', () => {
  // Arrange
  const records = sinkAt('info')

  // Act
  forwardSdkLog(undefined, 'info', 'moq-wasm::media::video_decoder', 'decoder configure failed', {
    error: new Error('codec unsupported'),
    group_id: 4n,
    config: { codec: 'avc1.42e01f' },
    rendition: 1
  })

  // Assert
  expect(records[0]).toMatchObject({
    target: 'moq-wasm::media::video_decoder',
    message: 'decoder configure failed',
    fields: { error: 'codec unsupported', group_id: '4', config: '{"codec":"avc1.42e01f"}', rendition: 1 }
  })
})

test("a worker's log message reaches the sink and other messages do not", () => {
  // Arrange
  const records = sinkAt('info')
  const message: WorkerLogMessage = {
    type: 'log',
    level: 'warn',
    target: 'moq-wasm::media::audio_decoder',
    message: 'decoder error',
    fields: { error: 'closed' }
  }

  // Act
  const log = receiveWorkerLog(message, undefined)
  const frame = receiveWorkerLog({ type: 'frame' }, undefined)

  // Assert
  expect([log, frame]).toEqual([true, false])
  expect(records).toMatchObject([{ level: 'warn', message: 'decoder error', fields: { error: 'closed' } }])
})

test('sdkLog writes one console line with its fields and hands the record to the sink', () => {
  // Arrange
  const records = sinkAt('info')
  const lines: unknown[][] = []
  const originalInfo = console.info
  console.info = (...args: unknown[]) => lines.push(args)

  // Act
  try {
    sdkLog(undefined, 'info', 'moq-wasm::core::session', 'session established', {
      transport: 'webtransport',
      duration_ms: 12
    })
  } finally {
    console.info = originalInfo
  }

  // Assert
  expect(lines).toEqual([['[moq-wasm::core::session] session established transport=webtransport duration_ms=12']])
  expect(records).toMatchObject([{ message: 'session established', fields: { transport: 'webtransport' } }])
})

test("a session's log goes to the sink with the session's newest token", () => {
  // Arrange
  const tokens = tokenSinkAt('info')
  const session = {}
  let token = 'jwt-a'
  bindLogToken(session, () => token)
  const workerMessage: WorkerLogMessage = {
    type: 'log',
    level: 'info',
    target: 'moq-wasm::media::video_encoder',
    message: 'configured',
    fields: {}
  }

  // Act
  forwardSdkLog(session, 'info', 'moq-wasm::media::live_player', 'catalog received')
  token = 'jwt-b'
  receiveWorkerLog(workerMessage, session)

  // Assert
  expect(tokens).toEqual(['jwt-a', 'jwt-b'])
})

test('a log of no session, or of a session without a token, goes without one', () => {
  // Arrange
  const tokens = tokenSinkAt('info')
  const session = {}
  bindLogToken(session, () => undefined)

  // Act
  forwardSdkLog(undefined, 'info', 'moq-wasm::media::player_controls', 'playout buffer changed')
  forwardSdkLog(session, 'info', 'moq-wasm::media::live_player', 'catalog received')
  forwardSdkLog({}, 'info', 'moq-wasm::media::live_player', 'catalog received')

  // Assert
  expect(tokens).toEqual([undefined, undefined, undefined])
})
