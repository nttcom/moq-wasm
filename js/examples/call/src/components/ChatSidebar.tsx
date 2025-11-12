import { FormEvent } from 'react'
import { ChatMessage } from '../types/chat'

interface ChatSidebarProps {
  messages: ChatMessage[]
  chatMessage: string
  onMessageChange: (value: string) => void
  onSend: (event?: FormEvent) => void
}

export function ChatSidebar({ messages, chatMessage, onMessageChange, onSend }: ChatSidebarProps) {
  return (
    <aside className="w-full border-t border-white/10 bg-gray-900/80 backdrop-blur px-4 py-6 lg:w-[28rem] lg:border-l lg:border-t-0 lg:px-6 lg:py-8">
      <div className="flex h-full flex-col">
        <h2 className="text-2xl font-semibold text-blue-200">Chat</h2>
        <div className="mt-4 flex h-full flex-col gap-4">
          <div className="flex-1 overflow-y-auto space-y-3 pr-1">
            {messages.length === 0 ? (
              <p className="text-blue-100">No messages yet.</p>
            ) : (
              messages.map((message, index) => (
                <div
                  key={`${message.timestamp}-${index}`}
                  className={`rounded-xl px-4 py-3 border border-white/10 ${
                    message.isLocal ? 'bg-blue-600/50 self-end' : 'bg-gray-900/70'
                  }`}
                >
                  <div className="flex items-center justify-between text-sm text-blue-200">
                    <span className="font-semibold">{message.sender}</span>
                    <span>{new Date(message.timestamp).toLocaleTimeString()}</span>
                  </div>
                  <p className="mt-2 text-base text-white whitespace-pre-wrap break-words">{message.text}</p>
                </div>
              ))
            )}
          </div>
          <form onSubmit={onSend} className="space-y-3 border-t border-white/10 pt-4">
            <label className="block text-blue-100 text-sm font-medium">
              Send Chat Message
              <input
                type="text"
                value={chatMessage}
                onChange={(event) => onMessageChange(event.target.value)}
                className="mt-2 w-full rounded-lg border border-blue-300/40 bg-white/5 px-4 py-3 text-base text-white placeholder-blue-200 focus:border-blue-200 focus:outline-none focus:ring-2 focus:ring-blue-300/50"
                placeholder="Type your message"
              />
            </label>
            <button
              type="submit"
              className="w-full rounded-lg bg-blue-600 px-4 py-2 font-semibold text-white transition hover:bg-blue-700 disabled:cursor-not-allowed disabled:bg-blue-600/50"
              disabled={!chatMessage.trim()}
            >
              Send
            </button>
          </form>
        </div>
      </div>
    </aside>
  )
}
