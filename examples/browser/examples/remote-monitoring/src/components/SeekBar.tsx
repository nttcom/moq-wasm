import React from 'react'

interface Props {
  replayableStartSeconds: number
  liveEdgeSeconds: number
  playheadSeconds: number
  onJump?: (seconds: number) => void
}

function fmt(secs: number): string {
  const s = Math.max(0, Math.round(secs))
  return `−${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}

/// The player's seek axis in capture time: from the oldest group the relay
/// still caches to the live edge.
export function SeekBar({ replayableStartSeconds, liveEdgeSeconds, playheadSeconds, onJump }: Props) {
  const totalSeconds = Math.max(liveEdgeSeconds - replayableStartSeconds, 1)
  const currentPct = Math.min(100, Math.max(0, ((playheadSeconds - replayableStartSeconds) / totalSeconds) * 100))
  const behindSeconds = liveEdgeSeconds - playheadSeconds

  const handleUpperClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!onJump) return
    const rect = e.currentTarget.getBoundingClientRect()
    const pct = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
    onJump(replayableStartSeconds + pct * totalSeconds)
  }

  return (
    <div className="space-y-2 font-mono text-xs select-none">
      <div>
        <div className="flex justify-between items-baseline mb-1.5">
          <span className="text-zinc-500">
            遡れる範囲：{fmt(totalSeconds)} 〜 LIVE
            {onJump && (
              <span className="text-zinc-600 ml-2" style={{ fontSize: '10px' }}>
                クリック＝ジャンプ
              </span>
            )}
          </span>
          <span className="text-amber-400 font-bold">遅れ {fmt(behindSeconds)}</span>
        </div>
        <div
          className={`relative h-11 rounded-lg overflow-hidden bg-zinc-800 ${onJump ? 'cursor-pointer' : 'cursor-default'}`}
          onClick={handleUpperClick}
        >
          <div
            className="absolute inset-0"
            style={{
              background: 'repeating-linear-gradient(90deg, rgba(47,108,168,.12) 0 7px, rgba(47,108,168,.04) 7px 14px)'
            }}
          />
          <div
            className="absolute top-0 bottom-0 w-0.5 bg-amber-400 z-20"
            style={{ left: `${currentPct}%`, transform: 'translateX(-50%)' }}
          />
          <div className="absolute top-0 bottom-0 right-0 w-0.5 bg-green-500 z-20" />
          <span className="absolute top-1.5 right-1.5 text-green-400 font-bold z-20" style={{ fontSize: '9px' }}>
            LIVE
          </span>
        </div>
      </div>
    </div>
  )
}
