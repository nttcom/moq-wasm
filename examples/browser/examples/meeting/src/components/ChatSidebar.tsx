import { FormEvent, useState } from 'react'
import type { TranscriptRecord } from '../media/transcript'
import { ChatMessage } from '../types/chat'

interface ChatSidebarProps {
  messages: ChatMessage[]
  chatMessage: string
  onMessageChange: (value: string) => void
  onSend: (event?: FormEvent) => void
  onToggle: () => void
  /// The room's transcript while captions are on.
  transcript: TranscriptRecord[] | undefined
}

export function ChatSidebar({
  messages,
  chatMessage,
  onMessageChange,
  onSend,
  onToggle,
  transcript
}: ChatSidebarProps) {
  const [tab, setTab] = useState<'chat' | 'transcript'>('chat')
  return (
    <aside className="h-full w-80 shrink-0 overflow-y-auto border-l border-white/10 bg-slate-900 px-4 py-4">
      <div className="flex h-full min-h-0 flex-col">
        <div className="flex items-center justify-between gap-2">
          <div className="flex gap-1">
            {transcript && (
              <>
                <SidebarTab label="Chat" active={tab === 'chat'} onClick={() => setTab('chat')} />
                <SidebarTab
                  label="Transcript"
                  testId="transcript-tab"
                  active={tab === 'transcript'}
                  onClick={() => setTab('transcript')}
                />
              </>
            )}
          </div>
          <button
            type="button"
            onClick={onToggle}
            className="rounded-md bg-white/10 px-3 py-1 text-xs font-semibold text-blue-100 transition hover:bg-white/20"
          >
            Close
          </button>
        </div>
        {tab === 'transcript' && transcript ? (
          <TranscriptPanel records={transcript} />
        ) : (
          <ChatPanel messages={messages} chatMessage={chatMessage} onMessageChange={onMessageChange} onSend={onSend} />
        )}
      </div>
    </aside>
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
    <div className="mt-4 flex h-full min-h-0 flex-col gap-4">
      <div className="flex-1 overflow-y-auto space-y-3 pr-1">
        {messages.length === 0 ? (
          <p className="text-blue-100">No messages yet.</p>
        ) : (
          messages.map((message, index) => (
            <div
              key={`${message.timestamp}-${index}`}
              data-testid="chat-message"
              className={`rounded-xl border border-white/10 px-4 py-3 ${
                message.isLocal ? 'self-end bg-blue-600/50' : 'bg-gray-900/70'
              }`}
            >
              <div className="flex items-center justify-between text-sm text-blue-200">
                <span className="font-semibold">{message.sender}</span>
                <span>{new Date(message.timestamp).toLocaleTimeString()}</span>
              </div>
              <p className="mt-2 whitespace-pre-wrap break-words text-base text-white">{message.text}</p>
            </div>
          ))
        )}
      </div>
      <form onSubmit={onSend} className="space-y-3 border-t border-white/10 pt-4">
        <label className="block text-sm font-medium text-blue-100">
          Send Chat Message
          <input
            type="text"
            data-testid="chat-message-input"
            value={chatMessage}
            onChange={(event) => onMessageChange(event.target.value)}
            className="mt-2 w-full rounded-lg border border-blue-300/40 bg-white/5 px-4 py-3 text-base text-white placeholder-blue-200 focus:border-blue-200 focus:outline-none focus:ring-2 focus:ring-blue-300/50"
            placeholder="Type your message"
          />
        </label>
        <button
          type="submit"
          data-testid="chat-send-button"
          className="w-full rounded-lg bg-blue-600 px-4 py-2 font-semibold text-white transition hover:bg-blue-700 disabled:cursor-not-allowed disabled:bg-blue-600/50"
          disabled={!chatMessage.trim()}
        >
          Send
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
      className={`rounded-md px-3 py-1 text-xs font-semibold transition ${
        active ? 'bg-blue-600 text-white' : 'bg-white/10 text-blue-100 hover:bg-white/20'
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
        <p className="text-blue-100">Nothing transcribed yet.</p>
      ) : (
        records.map((record) => (
          <div
            key={`${record.startedAtMs}-${record.speaker}`}
            data-testid="transcript-record"
            className="rounded-xl border border-white/10 bg-gray-900/70 px-4 py-3"
          >
            <div className="flex items-center justify-between text-sm text-blue-200">
              <span className="font-semibold">{record.speaker}</span>
              <span>{new Date(record.startedAtMs).toLocaleTimeString()}</span>
            </div>
            <p className="mt-2 whitespace-pre-wrap break-words text-base text-white">{record.text}</p>
          </div>
        ))
      )}
    </div>
  )
}
