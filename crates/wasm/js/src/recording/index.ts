/// The client of the recorder's `publish` request, kept out of `moq-wasm/media` so a page that
/// only plays live does not bundle it.
export { type OpenRecordingOptions, type Recording, RecorderError, openRecording } from './recording'
