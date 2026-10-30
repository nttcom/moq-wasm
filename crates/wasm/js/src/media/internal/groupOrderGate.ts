import type { SubgroupObjectWithLoc } from './jitterBufferTypes'
import { readLocHeader } from './loc'
import { isTerminalStatus } from './objectStatus'

const HOLD_MS = 100
const MICROS_PER_MILLI = 1_000

/// Maps a capture timestamp onto the local time by which its object has to
/// leave the gate to be heard on time.
export type ReleaseTimeline = { captureMicros: number; releaseAtMs: number }

type HeldGroup = { heldAtMs: number; objects: SubgroupObjectWithLoc[] }

/// Each group travels on its own stream, so when the tail of one group and the
/// head of the next are in flight together the two streams interleave and the
/// head lands first. Objects of a later group are held until the group before
/// them ends or the later group's head is due for release, then released in
/// order. So the tail is waited for as long as the playout buffer allows;
/// without a release timeline, for `HOLD_MS`. Objects of a group that has already been passed go straight
/// through.
export class GroupOrderGate {
  private open: bigint | undefined
  private readonly held = new Map<bigint, HeldGroup>()
  private timer: ReturnType<typeof setTimeout> | undefined
  private timeline: ReleaseTimeline | undefined

  constructor(private readonly release: (groupId: bigint, object: SubgroupObjectWithLoc) => void) {}

  push(groupId: bigint, object: SubgroupObjectWithLoc): void {
    if (this.open !== undefined && groupId > this.open) {
      const group = this.held.get(groupId)
      if (group) {
        group.objects.push(object)
      } else {
        this.held.set(groupId, { heldAtMs: performance.now(), objects: [object] })
        this.armTimer()
      }
      return
    }
    this.open ??= groupId
    this.release(groupId, object)
    if (groupId === this.open && isTerminalStatus(object.objectStatus)) {
      this.advance()
    }
  }

  setTimeline(timeline: ReleaseTimeline | undefined): void {
    this.timeline = timeline
    if (this.held.size > 0) {
      this.armTimer()
    }
  }

  private advance(): void {
    clearTimeout(this.timer)
    this.timer = undefined
    const next = this.earliestHeldGroupId()
    if (next === undefined) {
      this.open = undefined
      return
    }
    this.open = next
    const { objects } = this.held.get(next)!
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
    clearTimeout(this.timer)
    const next = this.earliestHeldGroupId()!
    const releaseAtMs = this.releaseAtMs(this.held.get(next)!)
    this.timer = setTimeout(
      () => {
        this.timer = undefined
        this.advance()
      },
      Math.max(0, releaseAtMs - performance.now())
    )
  }

  private releaseAtMs({ heldAtMs, objects }: HeldGroup): number {
    const captureMicros = readLocHeader(objects[0].locHeader).captureTimestampMicros
    if (this.timeline === undefined || captureMicros === undefined) {
      return heldAtMs + HOLD_MS
    }
    return this.timeline.releaseAtMs + (captureMicros - this.timeline.captureMicros) / MICROS_PER_MILLI
  }

  private earliestHeldGroupId(): bigint | undefined {
    const [earliest] = [...this.held.keys()].sort((left, right) => Number(left - right))
    return earliest
  }
}
