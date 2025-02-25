async function getUserMedia(constraints: MediaStreamConstraints): Promise<MediaStream> {
  try {
    const stream = await navigator.mediaDevices.getUserMedia(constraints)
    return stream
  } catch (error) {
    console.error('Error accessing media devices.', error)
    throw error
  }
}
