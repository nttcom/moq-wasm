import { expect, test } from '@playwright/test'
import { type RoomRecording, activeRecording, resolveRecorderUrl } from './recorderApi'

function recording(id: string, state: RoomRecording['state']): RoomRecording {
  return { id, namespace_prefix: 'anon/room', layout: 'combined', state, error: null, members: [] }
}

test('a configured recorder wins over the local default', () => {
  // Act / Assert
  expect(resolveRecorderUrl('?recorderUrl=http://recorder.example:8095/', 'https://localhost:4433')).toBe(
    'http://recorder.example:8095'
  )
})

test('a local relay implies the local recorder and a remote one implies none', () => {
  // Act / Assert
  expect(resolveRecorderUrl('', 'https://127.0.0.1:4433')).toBe('http://localhost:8095')
  expect(resolveRecorderUrl('', 'https://relay-1.moqt.research.skyway.io:443')).toBeNull()
  expect(resolveRecorderUrl('', 'not a url')).toBeNull()
})

test('only a recording that still writes files is active', () => {
  // Act / Assert
  expect(activeRecording([recording('a', 'finished'), recording('b', 'stopping')])?.id).toBe('b')
  expect(activeRecording([recording('a', 'finished'), recording('b', 'failed')])).toBeUndefined()
})
