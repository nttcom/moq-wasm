/// Shares one PUBLISH of a track between its callers. A failed PUBLISH is
/// dropped, so the next caller publishes the track again.
export class TrackPublication<T> {
  private pending: Promise<T> | null = null

  writer(publish: () => Promise<T>): Promise<T> {
    const pending = (this.pending ??= publish())
    return pending.catch((error: unknown) => {
      if (this.pending === pending) {
        this.pending = null
      }
      throw error
    })
  }

  reset(): void {
    this.pending = null
  }
}
