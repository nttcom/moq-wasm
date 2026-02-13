import { useState } from 'react'
import './App.css'
import { JoinRoomForm } from './components/JoinRoomForm'
import { CallRoom } from './components/CallRoom'
import { useLocalSession } from './hooks/useLocalSession'

function App() {
  const [joinError, setJoinError] = useState<Error | null>(null)
  const { session, connect, disconnect, isInitializing } = useLocalSession()

  const handleJoin = async (roomName: string, userName: string, relayUrl: string) => {
    try {
      await connect(roomName, userName, relayUrl)
      setJoinError(null)
    } catch (error) {
      console.error('Failed to join room:', error)
      const err = error instanceof Error ? error : new Error('Failed to join room')
      setJoinError(err)
    }
  }

  const handleLeave = async () => {
    try {
      await disconnect()
    } catch (error) {
      console.error('Failed to leave room:', error)
    }
  }

  return (
    <>
      {!session ? (
        <>
          <JoinRoomForm onJoin={handleJoin} />
          {joinError && <p className="text-red-500 text-center mt-4">{joinError.message}</p>}
          {isInitializing && <p className="text-blue-500 text-center mt-2">Connecting to MoQT relay...</p>}
        </>
      ) : (
        <CallRoom session={session} onLeave={handleLeave} />
      )}
    </>
  )
}

export default App
