class AudioProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
  }

  process(inputs, outputs, parameters) {
    // We expect one input, with two channels (stereo).
    const input = inputs[0];
    
    // The input may be empty if no audio is being sent.
    if (input.length < 1 || input[0].length === 0) {
      return true;
    }

    // Post the raw channel data back to the main thread.
    // We send a copy to avoid transferring ownership.
    this.port.postMessage({
      left: input[0].slice(),
      right: input.length > 1 ? input[1].slice() : input[0].slice(), // Handle mono input by duplicating channel
    });

    return true;
  }
}

registerProcessor('audio-processor', AudioProcessor);
