import { LocalMember, RemoteMember } from '../types/member'
import { RemoteMediaStreams } from '../types/media'
import { MediaStreamVideo, MediaStreamAudio } from './MediaStreamElements'
import { ReactNode } from 'react'

interface MemberGridProps {
  localMember: LocalMember
  remoteMembers: RemoteMember[]
  localVideoStream?: MediaStream | null
  localVideoBitrate?: number | null
  localAudioBitrate?: number | null
  remoteMedia: Map<string, RemoteMediaStreams>
}

export function MemberGrid({
  localMember,
  remoteMembers,
  localVideoStream,
  localVideoBitrate,
  localAudioBitrate,
  remoteMedia
}: MemberGridProps) {
  const localOverlay = renderStatsOverlay('Encoded', { bitrate: localVideoBitrate }, { bitrate: localAudioBitrate })
  return (
    <section className="mt-8 grid gap-6 md:grid-cols-2 xl:grid-cols-2">
      <MemberCard
        key="local-member"
        title={localMember.name}
        videoStream={localVideoStream}
        videoOverlay={localOverlay}
        audioStream={null}
        muted
        placeholder="Camera disabled"
        details={
          <>
            <DetailRow label="Name" value={localMember.name} />
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

      {remoteMembers.map((member) => {
        const media = remoteMedia.get(member.id)
        const remoteOverlay = renderStatsOverlay(
          'Received',
          { bitrate: media?.videoBitrateKbps, latency: media?.videoLatencyMs },
          { bitrate: media?.audioBitrateKbps, latency: media?.audioLatencyMs }
        )
        return (
          <MemberCard
            key={member.id}
            title={member.name}
            videoStream={media?.videoStream ?? null}
            videoOverlay={remoteOverlay}
            audioStream={media?.audioStream ?? null}
            placeholder="Awaiting video"
            details={
              <>
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
    </section>
  )
}

interface MemberCardProps {
  title: string
  videoStream?: MediaStream | null
  videoOverlay?: ReactNode
  audioStream?: MediaStream | null
  placeholder: string
  muted?: boolean
  details: ReactNode
}

function MemberCard({
  title,
  videoStream,
  videoOverlay,
  audioStream,
  placeholder,
  muted = false,
  details
}: MemberCardProps) {
  return (
    <div className="rounded-2xl border border-white/10 bg-white/10 p-6 shadow-xl backdrop-blur">
      <h3 className="mb-3 text-xl font-semibold text-white">{title}</h3>
      <MediaStreamVideo stream={videoStream} muted={muted} placeholder={placeholder} overlay={videoOverlay} />
      {audioStream && <MediaStreamAudio stream={audioStream} className="hidden" />}
      <div className="mt-4 space-y-3 text-sm text-blue-100">{details}</div>
    </div>
  )
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <p className="text-sm">
      <span className="font-medium text-blue-200">{label}:</span> {value}
    </p>
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
  latency?: number | null
}

function renderStatsOverlay(label: string, video?: TrackStats, audio?: TrackStats): ReactNode | undefined {
  const rows: string[] = []

  const formatBitrate = (kbps?: number | null) => {
    if (typeof kbps !== 'number') {
      return null
    }
    return `${kbps.toFixed(0)} kbps`
  }

  const formatLatency = (ms?: number | null) => {
    if (typeof ms !== 'number') {
      return null
    }
    return `${ms.toFixed(0)} ms`
  }

  const videoParts: string[] = []
  const videoBitrate = formatBitrate(video?.bitrate)
  if (videoBitrate) {
    videoParts.push(videoBitrate)
  }
  const videoLatency = formatLatency(video?.latency)
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
  const audioLatency = formatLatency(audio?.latency)
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
