const MAX_ENTRIES = 200
const LEVELS = ['log', 'info', 'warn', 'error'] as const

const panel = document.getElementById('logPanel')
if (panel) {
  for (const level of LEVELS) {
    const original = console[level].bind(console)
    console[level] = (...args: unknown[]) => {
      original(...args)
      append(panel, level === 'log' ? 'info' : level, args)
    }
  }
}

function append(panel: HTMLElement, level: string, args: unknown[]): void {
  const entry = document.createElement('div')
  entry.className = `log-entry log-${level}`
  const label = document.createElement('strong')
  label.textContent = level.toUpperCase()
  const text = document.createElement('span')
  text.textContent = ' ' + args.map(formatArg).join(' ')
  entry.append(label, text)
  panel.appendChild(entry)
  while (panel.childNodes.length > MAX_ENTRIES) {
    panel.firstChild?.remove()
  }
  panel.scrollTop = panel.scrollHeight
}

function formatArg(arg: unknown): string {
  if (typeof arg === 'string') {
    return arg
  }
  if (arg instanceof Error) {
    return arg.message
  }
  try {
    return JSON.stringify(arg)
  } catch {
    return String(arg)
  }
}
