import { useCallback, useEffect, useRef, useState } from 'react'
import { LocalSession } from '../session/localSession'

interface UseLocalSessionState {
  session: LocalSession | null
  isInitializing: boolean
}

export function useLocalSession() {
  const sessionRef = useRef<LocalSession | null>(null)
  const [{ session, isInitializing }, setState] = useState<UseLocalSessionState>({
    session: null,
    isInitializing: false
  })

  const connect = useCallback(async (roomName: string, userName: string, relayUrl: string) => {
    // The join form stays enabled while connecting, so a second submit must not open a second session.
    if (sessionRef.current) {
      return sessionRef.current
    }
    setState({ session: sessionRef.current, isInitializing: true })

    try {
      const newSession = new LocalSession({ roomName, userName, relayUrl })
      sessionRef.current = newSession
      await newSession.initialize()
      setState({ session: newSession, isInitializing: false })
      return newSession
    } catch (err) {
      const error = err instanceof Error ? err : new Error('Failed to initialize session')
      setState({ session: null, isInitializing: false })
      sessionRef.current = null
      throw error
    }
  }, [])

  const disconnect = useCallback(async () => {
    const currentSession = sessionRef.current
    if (!currentSession) {
      return
    }

    try {
      await currentSession.disconnect()
    } finally {
      sessionRef.current = null
      setState({ session: null, isInitializing: false })
    }
  }, [])

  useEffect(() => {
    return () => {
      if (sessionRef.current) {
        sessionRef.current.disconnect().catch((err) => {
          console.error('Failed to disconnect session on unmount:', err)
        })
        sessionRef.current = null
      }
    }
  }, [])

  return {
    session,
    connect,
    disconnect,
    isInitializing
  }
}
