const STEPS: Record<string, (index: number, count: number) => number> = {
  ArrowRight: (index, count) => (index + 1) % count,
  ArrowLeft: (index, count) => (index - 1 + count) % count,
  Home: () => 0,
  End: (_, count) => count - 1
}

/** The tab `key` moves to from tab `index` of `count`, or `undefined` when `key` does not move between tabs. */
export function nextTabIndex(key: string, index: number, count: number): number | undefined {
  return STEPS[key]?.(index, count)
}

/**
 * The `keydown` handler of a `role="tablist"`: Left and Right arrows, Home and End focus another tab and click it, so
 * the tablist's click handler selects it.
 */
export function moveBetweenTabs(event: { key: string; currentTarget: EventTarget | null; preventDefault(): void }) {
  const tabs = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('[role="tab"]')]
  const index = tabs.indexOf(document.activeElement as HTMLElement)
  const next = index < 0 ? undefined : nextTabIndex(event.key, index, tabs.length)
  if (next === undefined) return
  event.preventDefault()
  tabs[next].focus()
  tabs[next].click()
}

/** Keeps only the selected tab of `tablist` in the Tab order. Call after each selection. */
export function syncTabStops(tablist: HTMLElement): void {
  for (const tab of tablist.querySelectorAll<HTMLElement>('[role="tab"]')) {
    tab.tabIndex = tab.getAttribute('aria-selected') === 'true' ? 0 : -1
  }
}
