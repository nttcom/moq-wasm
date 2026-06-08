import { Button } from './ui/button'

interface Props {
  onStepBack: () => void
  onStepForward: () => void
  onReturnToLive: () => void
  canStepBack: boolean
  canStepForward: boolean
  disabled?: boolean
}

export function TransportControls({ onStepBack, onStepForward, onReturnToLive, canStepBack, canStepForward, disabled }: Props) {
  return (
    <div className="flex items-center gap-2">
      <Button
        size="sm"
        variant="outline"
        onClick={onStepBack}
        disabled={disabled || !canStepBack}
      >
        ◀▌ 1s 戻し
      </Button>
      <Button
        size="sm"
        variant="outline"
        onClick={onStepForward}
        disabled={disabled || !canStepForward}
      >
        1s 送り ▐▶
      </Button>
      <Button
        size="sm"
        variant="outline"
        onClick={onReturnToLive}
        disabled={disabled}
        className="ml-auto border-green-600 text-green-600 hover:bg-green-50"
      >
        ⤓ LIVE 復帰
      </Button>
    </div>
  )
}
