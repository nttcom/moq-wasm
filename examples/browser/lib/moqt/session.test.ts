import { expect, test } from '@playwright/test'
import { RequestError } from './moqtClient'
import { MoqNamespaceWatch, MoqSession, MoqSubscription } from './session'

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

test('a subscription closed while a read is in flight is freed only once the read returns', async () => {
  // Arrange
  let freed = 0
  const { promise: nextGroup, resolve: resolveNextGroup } = Promise.withResolvers<undefined>()
  const subscription = new MoqSubscription(
    wasmSubscription({
      nextGroup: () => nextGroup,
      free: () => {
        freed += 1
      }
    })
  )
  const reading = (async () => {
    for await (const group of subscription.groups()) {
      void group
    }
  })()

  // Act
  await subscription.close()
  const freedWhileReading = freed
  resolveNextGroup(undefined)
  await reading

  // Assert
  expect(freedWhileReading).toBe(0)
  expect(freed).toBe(1)
})

function wasmNamespaceWatch(overrides: Record<string, unknown>) {
  return {
    next: sequence([]),
    close: async () => {},
    free: () => {},
    ...overrides
  } as unknown as ConstructorParameters<typeof MoqNamespaceWatch>[0]
}

test('a namespace watch yields each announcement and withdrawal and frees the watch once it ends', async () => {
  // Arrange
  let freed = 0
  const change = (namespace: string[], active: boolean) => ({ namespace, active, free: () => {} })
  const watch = new MoqNamespaceWatch(
    wasmNamespaceWatch({
      next: sequence([change(['room', 'alice'], true), change(['room', 'alice'], false)]),
      free: () => {
        freed += 1
      }
    })
  )

  // Act
  const events = []
  for await (const event of watch) {
    events.push(event)
  }

  // Assert
  expect(events).toEqual([
    { kind: 'announced', namespace: ['room', 'alice'] },
    { kind: 'withdrawn', namespace: ['room', 'alice'] }
  ])
  expect(freed).toBe(1)
})

test('a namespace watch closed while a read is in flight is freed only once both calls return', async () => {
  // Arrange
  let freed = 0
  const { promise: next, resolve: resolveNext } = Promise.withResolvers<undefined>()
  const watch = new MoqNamespaceWatch(
    wasmNamespaceWatch({
      next: () => next,
      free: () => {
        freed += 1
      }
    })
  )
  const reading = (async () => {
    for await (const event of watch) {
      void event
    }
  })()

  // Act
  await watch.close()
  const freedWhileReading = freed
  resolveNext(undefined)
  await reading

  // Assert
  expect(freedWhileReading).toBe(0)
  expect(freed).toBe(1)
})

test('a subscribe passes its subscriber priority to the wasm session', async () => {
  // Arrange
  const priorities: unknown[] = []
  const session = new MoqSession({
    closed: new Promise(() => {}),
    subscribe: async (...args: unknown[]) => {
      priorities.push(args[4])
      return wasmSubscription({})
    }
  } as unknown as ConstructorParameters<typeof MoqSession>[0])

  // Act
  await session.subscribe(['room', 'alice'], 'audio', { priority: 0 })

  // Assert
  expect(priorities).toEqual([0])
})
