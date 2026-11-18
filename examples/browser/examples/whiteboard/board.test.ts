import { expect, test } from '@playwright/test'
import { Board, type BoardOp, type Stroke, decodeOp, encodeOp, strokeId } from './board'

function stroke(clock: number, author: string, points: number[] = [0.1, 0.1, 0.2, 0.2]): Stroke {
  return { id: strokeId({ clock, author }), color: '#000000', width: 4, points }
}

function boardOf(ops: readonly BoardOp[]): Board {
  const board = new Board()
  ops.forEach((op) => board.apply(op))
  return board
}

test('peers that apply the same operations in any order, any number of times, hold the same board', () => {
  // Arrange
  const ops: BoardOp[] = [
    { kind: 'stroke', stroke: stroke(1, 'alice', [0.1, 0.1]) },
    { kind: 'points', id: strokeId({ clock: 1, author: 'alice' }), from: 2, points: [0.3, 0.3] },
    { kind: 'stroke', stroke: stroke(2, 'bob') },
    { kind: 'erase', ids: [strokeId({ clock: 2, author: 'bob' })] },
    { kind: 'stroke', stroke: stroke(3, 'bob') },
    { kind: 'clear', at: { clock: 4, author: 'alice' } },
    { kind: 'stroke', stroke: stroke(5, 'bob') },
    { kind: 'stroke', stroke: stroke(4, 'bob') }
  ]
  const reversed = [...ops].reverse()
  const repeated = [...ops, ...reversed]

  // Act
  const snapshots = [ops, reversed, repeated].map((order) => boardOf(order).snapshot())

  // Assert: bob's clock-4 stroke wins the tie with alice's clear
  expect(snapshots[1]).toEqual(snapshots[0])
  expect(snapshots[2]).toEqual(snapshots[0])
  expect(
    boardOf(ops)
      .visibleStrokes()
      .map((visible) => visible.id)
  ).toEqual(['4@bob', '5@bob'])
})

test('a late joiner rebuilds the board by merging every peer snapshot it receives', () => {
  // Arrange: bob's copy of alice's stroke is shorter
  const alice = boardOf([
    { kind: 'stroke', stroke: stroke(1, 'alice', [0.1, 0.1, 0.2, 0.2, 0.3, 0.3]) },
    { kind: 'erase', ids: [strokeId({ clock: 2, author: 'bob' })] }
  ])
  const bob = boardOf([
    { kind: 'stroke', stroke: stroke(1, 'alice', [0.1, 0.1]) },
    { kind: 'stroke', stroke: stroke(2, 'bob') },
    { kind: 'stroke', stroke: stroke(3, 'bob') }
  ])

  // Act
  const joiner = boardOf([bob.snapshot(), alice.snapshot()])

  // Assert: the longer copy wins and the erase hides bob's stroke whichever snapshot came first
  expect(joiner.visibleStrokes()).toEqual([stroke(1, 'alice', [0.1, 0.1, 0.2, 0.2, 0.3, 0.3]), stroke(3, 'bob')])
  expect(boardOf([alice.snapshot(), bob.snapshot()]).snapshot()).toEqual(joiner.snapshot())
})

test('a local stamp comes after every operation the board has seen', () => {
  // Arrange
  const board = boardOf([{ kind: 'clear', at: { clock: 7, author: 'bob' } }])

  // Act
  const stamp = board.nextStamp('alice')
  const drawn = board.apply({ kind: 'stroke', stroke: stroke(stamp.clock, stamp.author) })

  // Assert: a stroke drawn after seeing the clear survives it
  expect(stamp).toEqual({ clock: 8, author: 'alice' })
  expect(drawn).toBe(true)
})

test('an operation survives encoding and a malformed payload is rejected', () => {
  // Arrange
  const op: BoardOp = {
    kind: 'snapshot',
    strokes: [stroke(1, 'alice')],
    erased: ['2@bob'],
    clearedAt: { clock: 1, author: 'bob' }
  }
  const malformed = [
    'not json',
    JSON.stringify({ kind: 'stroke', stroke: { ...stroke(1, 'alice'), points: [0.1] } }),
    JSON.stringify({ kind: 'stroke', stroke: { ...stroke(1, 'alice'), points: [] } }),
    JSON.stringify({ kind: 'points', id: '1@alice', from: 0, points: [2, 0] }),
    JSON.stringify({ kind: 'unknown' })
  ]

  // Act / Assert
  expect(decodeOp(encodeOp(op))).toEqual(op)
  for (const payload of malformed) {
    expect(decodeOp(new TextEncoder().encode(payload))).toBeUndefined()
  }
})
