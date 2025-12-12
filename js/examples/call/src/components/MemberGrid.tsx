import { LocalMember, RemoteMember } from '../types/member'
import { RemoteMediaStreams } from '../types/media'
import { MediaStreamVideo, MediaStreamAudio } from './MediaStreamElements'
import { ReactNode, useState } from 'react'
import type { VideoJitterConfig, AudioJitterConfig } from '../types/jitterBuffer'
import { VideoJitterBufferControls, AudioJitterBufferControls } from './JitterBufferControls'
import { Mic, MicOff, Monitor, Settings2, Video, VideoOff } from 'lucide-react'
import { DeviceSelector } from './DeviceSelector'
import type { VideoEncodingSettings } from '../types/videoEncoding'
import type { AudioEncodingSettings } from '../types/audioEncoding'
import type { CaptureSettingsState } from '../types/captureConstraints'
import { GetUserMediaForm } from './GetUserMediaForm'

interface MemberGridProps {
  localMember: LocalMember
  remoteMembers: RemoteMember[]
  localVideoStream?: MediaStream | null
  localVideoBitrate?: number | null
  localAudioBitrate?: number | null
  remoteMedia: Map<string, RemoteMediaStreams>
  videoJitterConfigs: Map<string, VideoJitterConfig>
  onChangeVideoJitterConfig: (userId: string, config: Partial<VideoJitterConfig>) => void
  audioJitterConfigs: Map<string, AudioJitterConfig>
  onChangeAudioJitterConfig: (userId: string, config: Partial<AudioJitterConfig>) => void
  onToggleCamera: () => void
  onToggleScreenShare: () => void
  onToggleMicrophone: () => void
  cameraEnabled: boolean
  screenShareEnabled: boolean
  microphoneEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  videoDevices: MediaDeviceInfo[]
  audioDevices: MediaDeviceInfo[]
  selectedVideoDeviceId: string | null
  selectedAudioDeviceId: string | null
  onSelectVideoDevice: (deviceId: string) => void
  onSelectAudioDevice: (deviceId: string) => void
  videoEncodingOptions: {
    codecOptions: { id: string; label: string; codec: string }[]
    resolutionOptions: { id: string; label: string; width: number; height: number }[]
    bitrateOptions: { id: string; label: string; bitrate: number }[]
  }
  selectedVideoEncoding: VideoEncodingSettings
  onSelectVideoEncoding: (settings: Partial<VideoEncodingSettings>) => void
  selectedScreenShareEncoding: VideoEncodingSettings
  onSelectScreenShareEncoding: (settings: Partial<VideoEncodingSettings>) => void
  videoEncoderError?: string | null
  audioEncodingOptions: {
    codecOptions: { id: string; label: string; codec: string }[]
    bitrateOptions: { id: string; label: string; bitrate: number }[]
    channelOptions: { id: string; label: string; channels: number }[]
  }
  selectedAudioEncoding: AudioEncodingSettings
  onSelectAudioEncoding: (settings: Partial<AudioEncodingSettings>) => void
  audioEncoderError?: string | null
  captureSettings: CaptureSettingsState
  onChangeCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  onApplyCaptureSettings: () => void
}

export function MemberGrid({
  localMember,
  remoteMembers,
  localVideoStream,
  localVideoBitrate,
  localAudioBitrate,
  remoteMedia,
  videoJitterConfigs,
  onChangeVideoJitterConfig,
  audioJitterConfigs,
  onChangeAudioJitterConfig,
  onToggleCamera,
  onToggleScreenShare,
  onToggleMicrophone,
  cameraEnabled,
  screenShareEnabled,
  microphoneEnabled,
  cameraBusy,
  microphoneBusy,
  videoDevices,
  audioDevices,
  selectedVideoDeviceId,
  selectedAudioDeviceId,
  onSelectVideoDevice,
  onSelectAudioDevice,
  videoEncodingOptions,
  selectedVideoEncoding,
  onSelectVideoEncoding,
  selectedScreenShareEncoding,
  onSelectScreenShareEncoding,
  videoEncoderError,
  audioEncodingOptions,
  selectedAudioEncoding,
  onSelectAudioEncoding,
  audioEncoderError,
  captureSettings,
  onChangeCaptureSettings,
  onApplyCaptureSettings
}: MemberGridProps) {
  const [isDeviceModalOpen, setIsDeviceModalOpen] = useState(false)
  const [jitterModalTarget, setJitterModalTarget] = useState<string | null>(null)

  const localOverlay = renderStatsOverlay('Encoded', { bitrate: localVideoBitrate }, { bitrate: localAudioBitrate })
  return (
    <section className="mt-8 grid gap-6 md:grid-cols-2 xl:grid-cols-2">
      <MemberCard
        key="local-member"
        title={localMember.name}
        headerActions={
          <div className="flex items-center gap-2">
            <ControlButton
              active={cameraEnabled}
              disabled={cameraBusy}
              onClick={onToggleCamera}
              activeLabel="Camera On"
              inactiveLabel="Camera Off"
              IconOn={Video}
              IconOff={VideoOff}
            />
            <ControlButton
              active={screenShareEnabled}
              disabled={cameraBusy}
              onClick={onToggleScreenShare}
              activeLabel="Sharing"
              inactiveLabel="Share"
              IconOn={Monitor}
              IconOff={Monitor}
            />
            <ControlButton
              active={microphoneEnabled}
              disabled={microphoneBusy}
              onClick={onToggleMicrophone}
              activeLabel="Mic On"
              inactiveLabel="Mic Off"
              IconOn={Mic}
              IconOff={MicOff}
            />
            <IconButton ariaLabel="Select devices" onClick={() => setIsDeviceModalOpen(true)} title="Device settings">
              <Settings2 className="h-4 w-4" />
            </IconButton>
          </div>
        }
        videoStream={localVideoStream}
        videoOverlay={localOverlay}
        audioStream={null}
        muted
        placeholder="Camera disabled"
        details={
          <>
            <TrackList
              title="Published Tracks"
              items={[
                { label: 'Chat', enabled: localMember.publishedTracks.chat },
                { label: 'Video', enabled: localMember.publishedTracks.video },
                { label: 'Audio', enabled: localMember.publishedTracks.audio }
              ]}
            />
          </>
        }
      />
      {isDeviceModalOpen && (
        <DeviceModal onClose={() => setIsDeviceModalOpen(false)}>
          <div className="space-y-4">
            <DeviceSelector
              label="Camera"
              devices={videoDevices}
              selectedDeviceId={selectedVideoDeviceId}
              onChange={onSelectVideoDevice}
            />
            <DeviceSelector
              label="Microphone"
              devices={audioDevices}
              selectedDeviceId={selectedAudioDeviceId}
              onChange={onSelectAudioDevice}
            />
            <GetUserMediaForm
              settings={captureSettings}
              onChange={onChangeCaptureSettings}
              onApply={onApplyCaptureSettings}
              cameraBusy={cameraBusy}
              microphoneBusy={microphoneBusy}
              resolutionOptions={videoEncodingOptions.resolutionOptions}
            />
            <div className="space-y-2 rounded-md border border-white/10 bg-white/5 p-3">
              <div className="text-sm font-semibold text-white">Video</div>
              <EncodingSelector
                codecOptions={videoEncodingOptions.codecOptions}
                resolutionOptions={videoEncodingOptions.resolutionOptions}
                bitrateOptions={videoEncodingOptions.bitrateOptions}
                selected={selectedVideoEncoding}
                onChange={onSelectVideoEncoding}
              />
            </div>
            <div className="space-y-2 rounded-md border border-white/10 bg-white/5 p-3">
              <div className="text-sm font-semibold text-white">Screenshare</div>
              <EncodingSelector
                codecOptions={videoEncodingOptions.codecOptions}
                resolutionOptions={videoEncodingOptions.resolutionOptions}
                bitrateOptions={videoEncodingOptions.bitrateOptions}
                selected={selectedScreenShareEncoding}
                onChange={onSelectScreenShareEncoding}
              />
            </div>
            {videoEncoderError && (
              <p className="rounded-md border border-red-400/60 bg-red-500/20 px-3 py-2 text-sm text-red-100">
                {videoEncoderError || 'Failed to initialize encoder. Please lower resolution/bitrate.'}
              </p>
            )}
            <div className="space-y-2 rounded-md border border-white/10 bg-white/5 p-3">
              <div className="text-sm font-semibold text-white">Audio</div>
              <AudioEncodingSelector
                codecOptions={audioEncodingOptions.codecOptions}
                bitrateOptions={audioEncodingOptions.bitrateOptions}
                channelOptions={audioEncodingOptions.channelOptions}
                selected={selectedAudioEncoding}
                onChange={onSelectAudioEncoding}
              />
            </div>
            {audioEncoderError && (
              <p className="rounded-md border border-red-400/60 bg-red-500/20 px-3 py-2 text-sm text-red-100">
                {audioEncoderError || 'Failed to initialize audio encoder. Please lower bitrate or change codec.'}
              </p>
            )}
          </div>
        </DeviceModal>
      )}

      {remoteMembers.map((member) => {
        const media = remoteMedia.get(member.id)
        const remoteOverlay = renderStatsOverlay(
          'Received',
          {
            bitrate: media?.videoBitrateKbps,
            latencyRender: media?.videoLatencyRenderMs,
            latencyReceive: media?.videoLatencyReceiveMs
          },
          {
            bitrate: media?.audioBitrateKbps,
            latencyRender: media?.audioLatencyRenderMs,
            latencyReceive: media?.audioLatencyReceiveMs
          }
        )
        return (
          <MemberCard
            key={member.id}
            title={member.name}
            headerActions={
              <IconButton
                ariaLabel="Configure jitter buffer"
                title="Jitter buffer settings"
                onClick={() => setJitterModalTarget(member.id)}
              >
                <Settings2 className="h-4 w-4" />
              </IconButton>
            }
            videoStream={media?.videoStream ?? null}
            videoOverlay={remoteOverlay}
            audioStream={media?.audioStream ?? null}
            placeholder="Awaiting video"
            details={
              <>
                <DetailRow label="Video Codec" value={media?.videoCodec ?? 'Unknown'} />
                <DetailRow
                  label="Resolution"
                  value={
                    media?.videoWidth && media?.videoHeight ? `${media.videoWidth}x${media.videoHeight}` : 'Unknown'
                  }
                />
                <TrackNamespaceList
                  title="Tracks Announced"
                  chat={member.announcedTracks.chat}
                  video={member.announcedTracks.video}
                  audio={member.announcedTracks.audio}
                />
                <SubscriptionStatusList
                  chat={member.subscribedTracks.chat}
                  video={member.subscribedTracks.video}
                  audio={member.subscribedTracks.audio}
                />
              </>
            }
          />
        )
      })}
      {jitterModalTarget && (
        <DeviceModal onClose={() => setJitterModalTarget(null)}>
          <div className="space-y-3">
            <h4 className="text-lg font-semibold text-white">
              Jitter Buffer ({findMemberName(remoteMembers, jitterModalTarget)})
            </h4>
            <VideoJitterBufferControls
              value={videoJitterConfigs.get(jitterModalTarget)}
              onChange={(config) => onChangeVideoJitterConfig(jitterModalTarget, config)}
            />
            <AudioJitterBufferControls
              value={audioJitterConfigs.get(jitterModalTarget)}
              onChange={(config) => onChangeAudioJitterConfig(jitterModalTarget, config)}
            />
          </div>
        </DeviceModal>
      )}
    </section>
  )
}

interface MemberCardProps {
  title: string
  headerActions?: ReactNode
  videoStream?: MediaStream | null
  videoOverlay?: ReactNode
  audioStream?: MediaStream | null
  placeholder: string
  muted?: boolean
  details: ReactNode
}

function MemberCard({
  title,
  headerActions,
  videoStream,
  videoOverlay,
  audioStream,
  placeholder,
  muted = false,
  details
}: MemberCardProps) {
  return (
    <div className="rounded-2xl border border-white/10 bg-white/10 p-6 shadow-xl backdrop-blur">
      <div className="mb-3 flex items-center justify-between gap-3">
        <h3 className="text-xl font-semibold text-white">{title}</h3>
        {headerActions}
      </div>
      <MediaStreamVideo stream={videoStream} muted={muted} placeholder={placeholder} overlay={videoOverlay} />
      {audioStream && <MediaStreamAudio stream={audioStream} className="hidden" />}
      <div className="mt-4 space-y-3 text-sm text-blue-100">{details}</div>
    </div>
  )
}

interface ControlButtonProps {
  active: boolean
  disabled: boolean
  onClick: () => void
  activeLabel: string
  inactiveLabel: string
  IconOn: typeof Mic
  IconOff: typeof MicOff
}

function ControlButton({ active, disabled, onClick, activeLabel, inactiveLabel, IconOn, IconOff }: ControlButtonProps) {
  const Icon = active ? IconOn : IconOff
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={`flex items-center gap-2 rounded-lg px-3 py-2 text-xs font-semibold transition ${
        active ? 'bg-blue-500 text-white hover:bg-blue-600' : 'bg-white/10 text-blue-100 hover:bg-white/20'
      } ${disabled ? 'cursor-not-allowed opacity-60' : ''}`}
    >
      <Icon className="h-4 w-4" />
      <span>{active ? activeLabel : inactiveLabel}</span>
    </button>
  )
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <p className="text-sm">
      <span className="font-medium text-blue-200">{label}:</span> {value}
    </p>
  )
}

function IconButton({
  ariaLabel,
  onClick,
  children,
  title
}: {
  ariaLabel: string
  onClick: () => void
  children: ReactNode
  title?: string
}) {
  return (
    <button
      type="button"
      aria-label={ariaLabel}
      title={title}
      onClick={onClick}
      className="rounded-lg bg-white/10 px-2 py-2 text-blue-100 transition hover:bg-white/20"
    >
      {children}
    </button>
  )
}

function DeviceModal({ onClose, children }: { onClose: () => void; children: ReactNode }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4 py-6">
      <div className="w-full max-w-md max-h-[85vh] overflow-y-auto rounded-2xl bg-slate-900 p-6 text-white shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h4 className="text-lg font-semibold">Select Devices</h4>
          <button
            onClick={onClose}
            className="rounded-md bg-white/10 px-3 py-1 text-sm text-blue-100 transition hover:bg-white/20"
          >
            Close
          </button>
        </div>
        {children}
      </div>
    </div>
  )
}

function findMemberName(members: RemoteMember[], id: string): string {
  return members.find((m) => m.id === id)?.name ?? id
}

function EncodingSelector({
  codecOptions,
  resolutionOptions,
  bitrateOptions,
  selected,
  onChange
}: {
  codecOptions: { id: string; label: string; codec: string }[]
  resolutionOptions: { id: string; label: string; width: number; height: number }[]
  bitrateOptions: { id: string; label: string; bitrate: number }[]
  selected: VideoEncodingSettings
  onChange: (settings: Partial<VideoEncodingSettings>) => void
}) {
  return (
    <div className="space-y-3 text-sm text-blue-100">
      <label className="flex flex-col gap-1">
        <span>Codec</span>
        <select
          className="rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          value={codecOptions.find((o) => o.codec === selected.codec)?.id ?? codecOptions[0]?.id}
          onChange={(e) => {
            const opt = codecOptions.find((o) => o.id === e.target.value)
            if (opt) {
              onChange({ codec: opt.codec })
            }
          }}
        >
          {codecOptions.map((opt) => (
            <option key={opt.id} value={opt.id} className="bg-slate-900 text-white">
              {opt.label}
            </option>
          ))}
        </select>
      </label>

      <label className="flex flex-col gap-1">
        <span>Resolution</span>
        <select
          className="rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          value={
            resolutionOptions.find((o) => o.width === selected.width && o.height === selected.height)?.id ??
            resolutionOptions[0]?.id
          }
          onChange={(e) => {
            const opt = resolutionOptions.find((o) => o.id === e.target.value)
            if (opt) {
              onChange({ width: opt.width, height: opt.height })
            }
          }}
        >
          {resolutionOptions.map((opt) => (
            <option key={opt.id} value={opt.id} className="bg-slate-900 text-white">
              {opt.label}
            </option>
          ))}
        </select>
      </label>

      <label className="flex flex-col gap-1">
        <span>Bitrate</span>
        <select
          className="rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          value={bitrateOptions.find((o) => o.bitrate === selected.bitrate)?.id ?? bitrateOptions[0]?.id}
          onChange={(e) => {
            const opt = bitrateOptions.find((o) => o.id === e.target.value)
            if (opt) {
              onChange({ bitrate: opt.bitrate })
            }
          }}
        >
          {bitrateOptions.map((opt) => (
            <option key={opt.id} value={opt.id} className="bg-slate-900 text-white">
              {opt.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  )
}

function AudioEncodingSelector({
  codecOptions,
  bitrateOptions,
  channelOptions,
  selected,
  onChange
}: {
  codecOptions: { id: string; label: string; codec: string }[]
  bitrateOptions: { id: string; label: string; bitrate: number }[]
  channelOptions: { id: string; label: string; channels: number }[]
  selected: AudioEncodingSettings
  onChange: (settings: Partial<AudioEncodingSettings>) => void
}) {
  return (
    <div className="space-y-3 text-sm text-blue-100">
      <label className="flex flex-col gap-1">
        <span>Codec</span>
        <select
          className="rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          value={codecOptions.find((o) => o.codec === selected.codec)?.id ?? codecOptions[0]?.id}
          onChange={(e) => {
            const opt = codecOptions.find((o) => o.id === e.target.value)
            if (opt) {
              onChange({ codec: opt.codec })
            }
          }}
        >
          {codecOptions.map((opt) => (
            <option key={opt.id} value={opt.id} className="bg-slate-900 text-white">
              {opt.label}
            </option>
          ))}
        </select>
      </label>

      <label className="flex flex-col gap-1">
        <span>Bitrate</span>
        <select
          className="rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          value={bitrateOptions.find((o) => o.bitrate === selected.bitrate)?.id ?? bitrateOptions[0]?.id}
          onChange={(e) => {
            const opt = bitrateOptions.find((o) => o.id === e.target.value)
            if (opt) {
              onChange({ bitrate: opt.bitrate })
            }
          }}
        >
          {bitrateOptions.map((opt) => (
            <option key={opt.id} value={opt.id} className="bg-slate-900 text-white">
              {opt.label}
            </option>
          ))}
        </select>
      </label>

      <label className="flex flex-col gap-1">
        <span>Channels</span>
        <select
          className="rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          value={channelOptions.find((o) => o.channels === selected.channels)?.id ?? channelOptions[0]?.id}
          onChange={(e) => {
            const opt = channelOptions.find((o) => o.id === e.target.value)
            if (opt) {
              onChange({ channels: opt.channels })
            }
          }}
        >
          {channelOptions.map((opt) => (
            <option key={opt.id} value={opt.id} className="bg-slate-900 text-white">
              {opt.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  )
}

function TrackList({ title, items }: { title: string; items: { label: string; enabled: boolean }[] }) {
  return (
    <div>
      <span className="font-medium text-blue-200">{title}:</span>
      <ul className="mt-1 space-y-1">
        {items.map((item) => (
          <li key={item.label}>
            {item.label}: {item.enabled ? 'Enabled' : 'Disabled'}
          </li>
        ))}
      </ul>
    </div>
  )
}

function TrackNamespaceList({
  title,
  chat,
  video,
  audio
}: {
  title: string
  chat: RemoteMember['announcedTracks']['chat']
  video: RemoteMember['announcedTracks']['video']
  audio: RemoteMember['announcedTracks']['audio']
}) {
  return (
    <div>
      <span className="font-medium text-blue-200">{title}:</span>
      <ul className="mt-1 space-y-1">
        <li>Chat: {chat.isAnnounced ? chat.trackNamespace?.join('/') : 'No'}</li>
        <li>Video: {video.isAnnounced ? video.trackNamespace?.join('/') : 'No'}</li>
        <li>Audio: {audio.isAnnounced ? audio.trackNamespace?.join('/') : 'No'}</li>
      </ul>
    </div>
  )
}

function SubscriptionStatusList({
  chat,
  video,
  audio
}: {
  chat: RemoteMember['subscribedTracks']['chat']
  video: RemoteMember['subscribedTracks']['video']
  audio: RemoteMember['subscribedTracks']['audio']
}) {
  return (
    <div>
      <span className="font-medium text-blue-200">Subscription Status:</span>
      <ul className="mt-1 space-y-1">
        <li>Chat: {describeSubscription(chat)}</li>
        <li>Video: {describeSubscription(video)}</li>
        <li>Audio: {describeSubscription(audio)}</li>
      </ul>
    </div>
  )
}

function describeSubscription(track: RemoteMember['subscribedTracks']['chat']): string {
  if (track.isSubscribed) return 'Subscribed'
  if (track.isSubscribing) return 'Subscribing...'
  return 'Not subscribed'
}

type TrackStats = {
  bitrate?: number | null
  latencyRender?: number | null
  latencyReceive?: number | null
}

function renderStatsOverlay(label: string, video?: TrackStats, audio?: TrackStats): ReactNode | undefined {
  const rows: string[] = []

  const formatBitrate = (kbps?: number | null) => {
    if (typeof kbps !== 'number') {
      return null
    }
    return `${kbps.toFixed(0)} kbps`
  }

  const formatLatencyPair = (renderMs?: number | null, receiveMs?: number | null) => {
    const renderText = typeof renderMs === 'number' ? `render ${renderMs.toFixed(0)} ms` : null
    const receiveText = typeof receiveMs === 'number' ? `recv ${receiveMs.toFixed(0)} ms` : null
    const parts = [renderText, receiveText].filter(Boolean)
    return parts.length ? parts.join(' / ') : null
  }

  const videoParts: string[] = []
  const videoBitrate = formatBitrate(video?.bitrate)
  if (videoBitrate) {
    videoParts.push(videoBitrate)
  }
  const videoLatency = formatLatencyPair(video?.latencyRender, video?.latencyReceive)
  if (videoLatency) {
    videoParts.push(videoLatency)
  }
  if (videoParts.length) {
    rows.push(`${label} Video: ${videoParts.join(' / ')}`)
  }

  const audioParts: string[] = []
  const audioBitrate = formatBitrate(audio?.bitrate)
  if (audioBitrate) {
    audioParts.push(audioBitrate)
  }
  const audioLatency = formatLatencyPair(audio?.latencyRender, audio?.latencyReceive)
  if (audioLatency) {
    audioParts.push(audioLatency)
  }
  if (audioParts.length) {
    rows.push(`${label} Audio: ${audioParts.join(' / ')}`)
  }

  if (!rows.length) {
    return undefined
  }

  return (
    <div className="flex flex-col text-right leading-tight">
      {rows.map((text) => (
        <span key={text}>{text}</span>
      ))}
    </div>
  )
}
