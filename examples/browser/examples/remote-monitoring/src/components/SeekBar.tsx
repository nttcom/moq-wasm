import React from 'react'

interface Props {
  firstGroupId: bigint
  entryGroupId: bigint   // fixed right edge (snapshot at REVIEW entry)
  latestGroupId: bigint  // live edge, used only for delay display
  currentGroupId: bigint
  fetchWindow: { startGroup: bigint; endGroup: bigint } | null
  onJump?: (groupId: bigint) => void
}

function fmt(secs: number): string {
  const s = Math.max(0, Math.round(secs))
  return `−${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}

export function SeekBar({ firstGroupId, entryGroupId, latestGroupId, currentGroupId, fetchWindow, onJump }: Props) {
  // Coarse bar: spans firstGroupId .. entryGroupId (fixed at review entry)
  const totalGroups = Number(entryGroupId - firstGroupId) || 1
  const currentOffset = Number(currentGroupId - firstGroupId)
  const currentPct = Math.min(100, Math.max(0, (currentOffset / totalGroups) * 100))

  // Fetch window within coarse bar
  const windowStartPct = fetchWindow
    ? Math.min(100, Math.max(0, (Number(fetchWindow.startGroup - firstGroupId) / totalGroups) * 100))
    : null
  const windowWidthPct = fetchWindow
    ? Math.min(100 - (windowStartPct ?? 0), (Number(fetchWindow.endGroup - fetchWindow.startGroup) / totalGroups) * 100)
    : null

  // Human-readable labels
  const delaySec = Number(latestGroupId - currentGroupId)
  const totalRange = Number(latestGroupId - firstGroupId)

  const handleUpperClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!onJump) return
    const rect = e.currentTarget.getBoundingClientRect()
    const pct = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
    const target = firstGroupId + BigInt(Math.round(pct * Number(entryGroupId - firstGroupId)))
    onJump(target)
  }

  return (
    <div className="space-y-2 font-mono text-xs select-none">
      {/* Coarse bar */}
      <div>
        <div className="flex justify-between items-baseline mb-1.5">
          <span className="text-zinc-500">
            遡れる範囲：{fmt(totalRange)} 〜 LIVE
            {onJump && <span className="text-zinc-600 ml-2" style={{ fontSize: '10px' }}>クリック＝ジャンプ</span>}
          </span>
          <span className="text-amber-400 font-bold">遅れ {fmt(delaySec)}</span>
        </div>
        <div
          className={`relative h-11 rounded-lg overflow-hidden bg-zinc-800 ${onJump ? 'cursor-pointer' : 'cursor-default'}`}
          onClick={handleUpperClick}
        >
          {/* striped available background */}
          <div
            className="absolute inset-0"
            style={{ background: 'repeating-linear-gradient(90deg, rgba(47,108,168,.12) 0 7px, rgba(47,108,168,.04) 7px 14px)' }}
          />
          {/* fetch window */}
          {windowStartPct !== null && windowWidthPct !== null && (
            <div
              className="absolute top-0 bottom-0 bg-amber-500/30 border-x border-amber-500/70 z-10"
              style={{ left: `${windowStartPct}%`, width: `${Math.max(windowWidthPct, 0.5)}%` }}
            />
          )}
          {/* playhead */}
          <div
            className="absolute top-0 bottom-0 w-0.5 bg-amber-400 z-20"
            style={{ left: `${currentPct}%`, transform: 'translateX(-50%)' }}
          />
          {/* LIVE edge */}
          <div className="absolute top-0 bottom-0 right-0 w-0.5 bg-green-500 z-20" />
          <span
            className="absolute top-1.5 right-1.5 text-green-400 font-bold z-20"
            style={{ fontSize: '9px' }}
          >
            LIVE
          </span>
        </div>
      </div>
    </div>
  )
}
