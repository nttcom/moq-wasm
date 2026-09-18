export interface CameraCaptureConstraints {
  width?: number
  height?: number
  frameRate?: number
}

export interface AudioCaptureConstraints {
  echoCancellation?: boolean
  noiseSuppression?: boolean
  autoGainControl?: boolean
}

export interface CaptureSettingsState {
  videoEnabled: boolean
  audioEnabled: boolean
  width: number
  height: number
  frameRate: number
  echoCancellation: boolean
  noiseSuppression: boolean
  autoGainControl: boolean
}
