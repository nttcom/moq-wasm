(async () => {
  const codecs = [
    "opus",
    "mp4a.40.2", // AAC-LC
    "flac",
    "vorbis",
    "pcm",
  ];
  console.log("🎧 AudioEncoder support check:\n");
  for (const codec of codecs) {
    try {
      const support = await AudioEncoder.isConfigSupported({
        codec,
        sampleRate: 48000,
        numberOfChannels: 2,
      });
      console.log(codec, support.supported ? "✅ supported" : "❌ not supported");
    } catch (e) {
      console.log(codec, "❌ error:", e.message);
    }
  }
})();