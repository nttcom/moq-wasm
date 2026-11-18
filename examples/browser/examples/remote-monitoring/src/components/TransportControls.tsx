import { Radio, StepBack, StepForward } from 'lucide-react'
import { Button } from '../../../../lib/ui/button'

interface Props {
  onStepBack: () => void
  onStepForward: () => void
  onReturnToLive: () => void
  canStepBack: boolean
  canStepForward: boolean
  disabled?: boolean
}

export function TransportControls({
  onStepBack,
  onStepForward,
  onReturnToLive,
  canStepBack,
  canStepForward,
  disabled
}: Props) {
  return (
    <div className="flex items-center gap-2">
      <Button size="sm" variant="outline" title="1 秒戻す" onClick={onStepBack} disabled={disabled || !canStepBack}>
        <StepBack className="h-4 w-4" />
        1s
      </Button>
      <Button
        size="sm"
        variant="outline"
        title="1 秒進める"
        onClick={onStepForward}
        disabled={disabled || !canStepForward}
      >
        1s
        <StepForward className="h-4 w-4" />
      </Button>
      <Button
        size="sm"
        variant="outline"
        onClick={onReturnToLive}
        disabled={disabled}
        title="LIVE に戻る"
        className="ml-auto border-success/50 text-success hover:bg-success-soft"
      >
        <Radio className="h-4 w-4" />
        LIVE
      </Button>
    </div>
  )
}
