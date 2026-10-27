import { expect, test } from '@playwright/test'
import { RequestError } from './moqtClient'
import { MoqSession, MoqSubscription } from './session'

type Freed = { freed: number }

function wasmObject(objectId: bigint, payload: number[], freed: Freed, status?: number) {
  return {
    groupId: 4n,
    subgroupId: 0n,
    objectId,
    status,
    payload: new Uint8Array(payload),
    locHeader: [],
    free: () => {
      freed.freed += 1
    }
  }
}

/// Hands out `items` one per `next()`, then `undefined`.
function sequence<T>(items: T[]): () => Promise<T | undefined> {
  return async () => items.shift()
}

function wasmSubscription(overrides: Record<string, unknown>) {
  return {
    largest: undefined,
    done: new Promise(() => {}),
    next: sequence([]),
    nextGroup: sequence([]),
    setForward: async () => {},
    endBeforeGroup: async () => {},
    close: async () => {},
    free: () => {},
    ...overrides
  } as unknown as ConstructorParameters<typeof MoqSubscription>[0]
}

test('a subscription yields copies of its objects and frees each wasm object', async () => {
  // Arrange
  const freed = { freed: 0 }
  const subscription = new MoqSubscription(
    wasmSubscription({ next: sequence([wasmObject(0n, [1, 2], freed), wasmObject(1n, [3], freed)]) })
  )

  // Act
  const objects = []
  for await (const object of subscription) {
    objects.push(object)
  }

  // Assert
  expect(objects.map((object) => [object.objectId, [...object.objectPayload], object.objectPayloadLength])).toEqual([
    [0n, [1, 2], 2],
    [1n, [3], 1]
  ])
  expect(freed.freed).toBe(2)
})

test('the objects of a group keep their status and the group is freed once read', async () => {
  // Arrange
  const freed = { freed: 0 }
  const groupFreed = { freed: 0 }
  const group = {
    groupId: 4n,
    subgroupId: 0n,
    next: sequence([wasmObject(0n, [1], freed), wasmObject(1n, [], freed, 3)]),
    free: () => {
      groupFreed.freed += 1
    }
  }
  const subscription = new MoqSubscription(wasmSubscription({ nextGroup: sequence([group]) }))

  // Act
  const statuses = []
  for await (const received of subscription.groups()) {
    for await (const object of received) {
      statuses.push([received.groupId, object.objectId, object.objectStatus])
    }
  }

  // Assert
  expect(statuses).toEqual([
    [4n, 0n, undefined],
    [4n, 1n, 3]
  ])
  expect(groupFreed.freed).toBe(1)
})

test('a request the relay rejects throws a RequestError with its code', async () => {
  // Arrange
  const rejection = Object.assign(new Error('track does not exist'), { code: 0x4n })
  const session = new MoqSession({
    closed: new Promise(() => {}),
    subscribe: async () => {
      throw rejection
    }
  } as unknown as ConstructorParameters<typeof MoqSession>[0])

  // Act
  const error = await session.subscribe(['live'], 'video').catch((error: unknown) => error)

  // Assert
  expect(error).toBeInstanceOf(RequestError)
  expect((error as RequestError).errorCode).toBe(0x4n)
})
