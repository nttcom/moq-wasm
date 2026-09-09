const WINDOW_MS = 60_000;

export class RateLimiter {
  #limit;
  #windows = new Map();

  constructor(limitPerMinute) {
    this.#limit = limitPerMinute;
  }

  allow(key, nowMs = Date.now()) {
    const windowStart = nowMs - (nowMs % WINDOW_MS);
    const entry = this.#windows.get(key);
    if (!entry || entry.windowStart !== windowStart) {
      this.#windows.set(key, { windowStart, count: 1 });
      this.#evict(windowStart);
      return true;
    }
    if (entry.count >= this.#limit) {
      return false;
    }
    entry.count += 1;
    return true;
  }

  #evict(currentWindowStart) {
    for (const [key, entry] of this.#windows) {
      if (entry.windowStart !== currentWindowStart) {
        this.#windows.delete(key);
      }
    }
  }
}
