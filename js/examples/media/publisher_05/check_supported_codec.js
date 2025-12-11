(async () => {

  const video_codecs = [
    "avc1.42E01E", // H.264 Baseline
    "avc1.640028", // H.264 High
    "vp8",
    "vp09.00.10.08", // VP9 profile 0
    "av01.0.05M.08", // AV1 Main 8bit
    "hvc1.1.6.L93.B0", // HEVC / H.265 (Safari など)
  ];

  console.log("====================================")
  console.log("🎥 VideoEncoder support check:\n");

  for (const codec of video_codecs) {
    try {
      const support = await VideoEncoder.isConfigSupported({
        codec,
        width: 1920,
        height: 1080,
        bitrate: 5_000_000,
        framerate: 30,
      });
      console.log(codec, support.supported ? "✅ supported" : "❌ not supported");
    } catch (e) {
      console.log(codec, "❌ error:", e.message);
    }
  }

  console.log("------------------------------------")

  console.log("🎧 AudioEncoder support check:\n");
  const audio_codecs = [
    "opus",
    "mp4a.40.2", // AAC-LC
    "flac",
    "vorbis",
    "pcm",
  ];

  for (const codec of audio_codecs) {
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
  console.log("====================================")
})();