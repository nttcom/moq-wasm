import { useCallback, useEffect, useRef, useState } from 'react'
import { LocalSession } from '../session/localSession'

interface UseLocalSessionState {
  session: LocalSession | null
  isInitializing: boolean
  error: Error | null
}

export function useLocalSession() {
  const sessionRef = useRef<LocalSession | null>(null)
  const [{ session, isInitializing, error }, setState] = useState<UseLocalSessionState>({
    session: null,
    isInitializing: false,
    error: null
  })

  const connect = useCallback(async (roomName: string, userName: string) => {
    setState({ session: sessionRef.current, isInitializing: true, error: null })

    try {
      const newSession = new LocalSession({ roomName, userName })
      sessionRef.current = newSession
      await newSession.initialize()
      setState({ session: newSession, isInitializing: false, error: null })
      return newSession
    } catch (err) {
      const error = err instanceof Error ? err : new Error('Failed to initialize session')
      setState({ session: null, isInitializing: false, error })
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
      setState({ session: null, isInitializing: false, error: null })
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
    isInitializing,
    error
  }
}
