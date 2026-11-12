const MAX_BYTES = 64 * 1024 * 1024

type Entry<T> = { result: Promise<T>; bytes: number }

/// What a player has fetched of single groups, kept across its reviews so a
/// seek to a group it holds needs no FETCH; a fetch still under way is shared
/// as well. A group's objects never change, so an entry stays valid until the
/// tracks change. The oldest entries leave once the held payloads pass
/// `MAX_BYTES`.
export class ReviewGroupStore<T> {
  private tracks: string | undefined
  private generation = 0
  private heldBytes = 0
  private readonly entries = new Map<bigint, Entry<T>>()

  /// `bytesOf` returns the payload size of a result worth keeping and 0 for
  /// one that is not, such as a failure.
  constructor(private readonly bytesOf: (result: T) => number) {}

  /// `fetch` is given whether its result is still wanted: it no longer is once
  /// the store has been cleared or has moved to other tracks. `tracks` names
  /// the tracks the result is fetched from.
  fetched(tracks: string, groupId: bigint, fetch: (isCurrent: () => boolean) => Promise<T>): Promise<T> {
    if (tracks !== this.tracks) {
      this.clear()
      this.tracks = tracks
    }
    const held = this.entries.get(groupId)
    if (held) {
      return held.result
    }
    const generation = this.generation
    const entry: Entry<T> = { result: fetch(() => generation === this.generation), bytes: 0 }
    this.entries.set(groupId, entry)
    void entry.result.then(
      (result) => this.settle(groupId, entry, this.bytesOf(result)),
      () => this.settle(groupId, entry, 0)
    )
    return entry.result
  }

  clear(): void {
    this.generation += 1
    this.entries.clear()
    this.heldBytes = 0
    this.tracks = undefined
  }

  private settle(groupId: bigint, entry: Entry<T>, bytes: number): void {
    if (this.entries.get(groupId) !== entry) {
      return
    }
    if (bytes === 0) {
      this.entries.delete(groupId)
      return
    }
    entry.bytes = bytes
    this.heldBytes += bytes
    for (const [oldestGroupId, oldest] of this.entries) {
      if (this.heldBytes <= MAX_BYTES) {
        break
      }
      this.entries.delete(oldestGroupId)
      this.heldBytes -= oldest.bytes
    }
  }
}
