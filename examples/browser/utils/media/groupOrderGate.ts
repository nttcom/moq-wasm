import type { SubgroupObjectWithLoc } from './jitterBufferTypes'
import { isTerminalStatus } from './objectStatus'

const HOLD_MS = 100

/// Each group travels on its own stream, so when the tail of one group and the
/// head of the next are in flight together the two streams interleave and the
/// head lands first. Objects of a later group are held until the group before
/// them has ended, or for `HOLD_MS` when its end does not arrive, and released
/// in order. Objects of a group that has already been passed go straight
/// through.
export class GroupOrderGate {
  private open: bigint | undefined
  private readonly held = new Map<bigint, SubgroupObjectWithLoc[]>()
  private timer: ReturnType<typeof setTimeout> | undefined

  constructor(private readonly release: (groupId: bigint, object: SubgroupObjectWithLoc) => void) {}

  push(groupId: bigint, object: SubgroupObjectWithLoc): void {
    if (this.open !== undefined && groupId > this.open) {
      this.held.set(groupId, [...(this.held.get(groupId) ?? []), object])
      this.armTimer()
      return
    }
    this.open ??= groupId
    this.release(groupId, object)
    if (groupId === this.open && isTerminalStatus(object.objectStatus)) {
      this.advance()
    }
  }

  private advance(): void {
    clearTimeout(this.timer)
    this.timer = undefined
    const [next] = [...this.held.keys()].sort((left, right) => Number(left - right))
    if (next === undefined) {
      this.open = undefined
      return
    }
    this.open = next
    const objects = this.held.get(next) ?? []
    this.held.delete(next)
    for (const object of objects) {
      this.release(next, object)
    }
    if (objects.some((object) => isTerminalStatus(object.objectStatus))) {
      this.advance()
    } else if (this.held.size > 0) {
      this.armTimer()
    }
  }

  private armTimer(): void {
    this.timer ??= setTimeout(() => {
      this.timer = undefined
      this.advance()
    }, HOLD_MS)
  }
}
