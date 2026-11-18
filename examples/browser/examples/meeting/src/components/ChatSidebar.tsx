import { FormEvent, useState } from 'react'
import { MessageSquare, SendHorizontal, X } from 'lucide-react'
import { ASR_CHOICES, type CaptionsControls } from '../hooks/useCaptions'
import type { RoomRecordingControls } from '../hooks/useRoomRecording'
import type { TranscriptRecord } from '../media/transcript'
import { ChatMessage } from '../types/chat'
import { RecordPanel } from './RecordPanel'

interface ChatSidebarProps {
  messages: ChatMessage[]
  chatMessage: string
  onMessageChange: (value: string) => void
  onSend: (event?: FormEvent) => void
  onToggle: () => void
  /// `null` in a room the transcriber does not serve.
  captions: CaptionsControls | null
  /// Whether the transcriber is in the room.
  transcribing: boolean
  /// `null` while the recorder has not answered.
  recording: RoomRecordingControls | null
}

export function ChatSidebar({
  messages,
  chatMessage,
  onMessageChange,
  onSend,
  onToggle,
  captions,
  transcribing,
  recording
}: ChatSidebarProps) {
  const [tab, setTab] = useState<'chat' | 'transcription' | 'record'>('chat')
  return (
    <aside className="flex h-full w-80 shrink-0 flex-col border-l border-line bg-surface">
      <div className="flex items-center justify-between gap-2 border-b border-line px-3 py-2.5">
        <div className="flex gap-0.5 rounded-lg border border-line bg-sunken p-0.5">
          <SidebarTab label="Chat" active={tab === 'chat'} onClick={() => setTab('chat')} />
          <SidebarTab
            label="Transcription"
            testId="transcript-tab"
            active={tab === 'transcription'}
            onClick={() => setTab('transcription')}
          />
          <SidebarTab label="Record" testId="record-tab" active={tab === 'record'} onClick={() => setTab('record')} />
        </div>
        <button
          type="button"
          onClick={onToggle}
          aria-label="Close"
          title="Close"
          className="inline-flex h-8 w-8 cursor-pointer items-center justify-center rounded-md text-muted transition hover:bg-hover hover:text-fg"
        >
          <X className="h-4 w-4" />
        </button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col px-4 pb-4">
        {tab === 'record' ? (
          <RecordPanel recording={recording} />
        ) : tab === 'transcription' ? (
          transcribing && captions ? (
            <TranscriptPanel records={captions.records} />
          ) : (
            <TranscriptionNotice captions={captions} />
          )
        ) : (
          <ChatPanel messages={messages} chatMessage={chatMessage} onMessageChange={onMessageChange} onSend={onSend} />
        )}
      </div>
    </aside>
  )
}

function TranscriptionNotice({ captions }: { captions: CaptionsControls | null }) {
  if (!captions) {
    return <p className="mt-4 text-sm text-muted">Transcription is not available in this room</p>
  }
  return (
    <div className="mt-4 space-y-3">
      <p className="rounded-lg border border-line bg-subtle px-4 py-3 text-sm text-muted">
        {captions.active ? 'Starting transcription…' : 'Transcription has not started yet'}
      </p>
      {!captions.active && (
        <>
          {captions.error && <p className="text-xs text-danger">{captions.error}</p>}
          <div className="text-xs font-semibold uppercase tracking-wide text-muted">Start transcription</div>
          <div className="space-y-2">
            {ASR_CHOICES.map(({ asr, label }) => (
              <button
                key={asr}
                type="button"
                onClick={() => captions.start(asr)}
                className="w-full cursor-pointer rounded-lg border border-line-strong bg-raised px-4 py-2 text-left text-sm font-semibold text-fg transition hover:bg-hover"
              >
                {label}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  )
}

function ChatPanel({
  messages,
  chatMessage,
  onMessageChange,
  onSend
}: {
  messages: ChatMessage[]
  chatMessage: string
  onMessageChange: (value: string) => void
  onSend: (event?: FormEvent) => void
}) {
  return (
    <div className="mt-4 flex min-h-0 flex-1 flex-col gap-4">
      <div className="flex flex-1 flex-col gap-2 overflow-y-auto pr-1">
        {messages.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-center text-sm text-faint">
            <MessageSquare className="h-6 w-6" />
            <p>No messages yet.</p>
          </div>
        ) : (
          messages.map((message, index) => (
            <div
              key={`${message.timestamp}-${index}`}
              data-testid="chat-message"
              className={`max-w-[88%] rounded-lg border px-3 py-2 ${
                message.isLocal ? 'self-end border-accent-line bg-accent-soft' : 'self-start border-line bg-sunken'
              }`}
            >
              <div className="flex items-center justify-between gap-3 text-xs text-faint">
                <span className="font-semibold text-muted">{message.sender}</span>
                <span className="tabular-nums">{new Date(message.timestamp).toLocaleTimeString()}</span>
              </div>
              <p className="mt-1 whitespace-pre-wrap break-words text-sm text-fg">{message.text}</p>
            </div>
          ))
        )}
      </div>
      <form onSubmit={onSend} className="flex items-center gap-2 border-t border-line pt-3">
        <label className="min-w-0 flex-1">
          <span className="sr-only">Send Chat Message</span>
          <input
            type="text"
            data-testid="chat-message-input"
            value={chatMessage}
            onChange={(event) => onMessageChange(event.target.value)}
            className="h-10 w-full rounded-md border border-line-strong bg-sunken px-3 text-sm text-fg placeholder:text-faint focus:border-accent focus:outline-none focus:ring-3 focus:ring-accent/25"
            placeholder="Type your message"
          />
        </label>
        <button
          type="submit"
          data-testid="chat-send-button"
          aria-label="Send"
          title="Send"
          className="inline-flex h-10 w-10 shrink-0 cursor-pointer items-center justify-center rounded-md bg-accent text-on-accent transition hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-40"
          disabled={!chatMessage.trim()}
        >
          <SendHorizontal className="h-4 w-4" />
        </button>
      </form>
    </div>
  )
}

function SidebarTab({
  label,
  testId,
  active,
  onClick
}: {
  label: string
  testId?: string
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      data-testid={testId}
      onClick={onClick}
      aria-pressed={active}
      className={`cursor-pointer rounded-md px-2.5 py-1 text-xs font-semibold transition ${
        active ? 'bg-raised text-fg shadow-sm ring-1 ring-line-strong' : 'text-muted hover:text-fg'
      }`}
    >
      {label}
    </button>
  )
}

function TranscriptPanel({ records }: { records: TranscriptRecord[] }) {
  return (
    <div className="mt-4 min-h-0 flex-1 space-y-3 overflow-y-auto pr-1">
      {records.length === 0 ? (
        <p className="text-sm text-faint">Nothing transcribed yet.</p>
      ) : (
        records.map((record) => (
          <div
            key={`${record.startedAtMs}-${record.speaker}`}
            data-testid="transcript-record"
            className="rounded-lg border border-line bg-sunken px-3 py-2"
          >
            <div className="flex items-center justify-between gap-3 text-xs text-faint">
              <span className="font-semibold text-muted">{record.speaker}</span>
              <span className="tabular-nums">{new Date(record.startedAtMs).toLocaleTimeString()}</span>
            </div>
            <p className="mt-1 whitespace-pre-wrap break-words text-sm text-fg">{record.text}</p>
          </div>
        ))
      )}
    </div>
  )
}
