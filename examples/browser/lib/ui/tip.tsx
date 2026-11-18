import { Info } from 'lucide-react'

/// The ⓘ of the `.tip` rule in utils/theme.css: `note` shows while the pointer is over it or it has focus.
/// `end` opens it leftward, for a tip at the right end of a row.
export function Tip({ note, end = false }: { note: string; end?: boolean }) {
  return (
    <span className={end ? 'tip end' : 'tip'} tabIndex={0} role="img" aria-label={note} data-tip={note}>
      <Info className="h-3.5 w-3.5" />
    </span>
  )
}
