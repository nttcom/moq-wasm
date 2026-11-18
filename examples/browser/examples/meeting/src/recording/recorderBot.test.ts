import { expect, test } from '@playwright/test'
import { activeRecording } from './recorderBot'

test('a recorder bot that is starting, running or stopping makes the active recording', () => {
  // Act / Assert
  expect(activeRecording({ id: 'a', state: 'starting' })).toEqual({ id: 'a', state: 'starting' })
  expect(activeRecording({ id: 'a', state: 'running' })).toEqual({ id: 'a', state: 'recording' })
  expect(activeRecording({ id: 'a', state: 'stopping' })).toEqual({ id: 'a', state: 'stopping' })
  expect(activeRecording({ id: 'a', state: 'finished' })).toBeUndefined()
  expect(activeRecording({ id: 'a', state: 'failed' })).toBeUndefined()
  expect(activeRecording(undefined)).toBeUndefined()
})
