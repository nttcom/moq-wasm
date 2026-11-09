import { expect, test } from '@playwright/test'
import type { LogRecord } from './init'
import { type WorkerLogMessage, forwardSdkLog, receiveWorkerLog, sdkLog, setSdkLogSink } from './log'

function sinkAt(level: LogRecord['level']): LogRecord[] {
  const records: LogRecord[] = []
  setSdkLogSink((record) => records.push(record), level)
  return records
}

test('a log below the sink level is not handed to it', () => {
  // Arrange
  const records = sinkAt('warn')

  // Act
  forwardSdkLog('info', 'moq-wasm::media::live_player', 'adaptive quality changed')
  forwardSdkLog('error', 'moq-wasm::media::live_player', 'catalog failed')

  // Assert
  expect(records.map(({ level, message }) => [level, message])).toEqual([['error', 'catalog failed']])
})

test('fields that are not a string, number or boolean are flattened', () => {
  // Arrange
  const records = sinkAt('info')

  // Act
  forwardSdkLog('info', 'moq-wasm::media::video_decoder', 'decoder configure failed', {
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
  const log = receiveWorkerLog(message)
  const frame = receiveWorkerLog({ type: 'frame' })

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
    sdkLog('info', 'moq-wasm::core::session', 'session established', { transport: 'webtransport', duration_ms: 12 })
  } finally {
    console.info = originalInfo
  }

  // Assert
  expect(lines).toEqual([['[moq-wasm::core::session] session established transport=webtransport duration_ms=12']])
  expect(records).toMatchObject([{ message: 'session established', fields: { transport: 'webtransport' } }])
})
