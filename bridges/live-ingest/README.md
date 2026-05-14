## Run

```bash
make live-ingest
```

## Publish RTMP From ffmpeg

```bash
ffmpeg -loglevel info -re \
  -f lavfi -i "testsrc=size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=1000:sample_rate=48000" \
  -c:v libx264 -profile:v high -level:v 4.0 -preset veryfast -tune zerolatency \
  -pix_fmt yuv420p \
  -f flv "rtmp://localhost:1935/live/test"
```

```bash
ffmpeg -re \
  -f lavfi -i "testsrc=size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=1000:sample_rate=48000" \
  -f flv rtmp://localhost:1935/live/test
```

### Playback With ffplay

```bash
ffplay recordings/live_test.flv
```

### Publish SRT From ffmpeg

```bash
ffmpeg -re \
  -f lavfi -i "testsrc=size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=1000:sample_rate=48000" \
  -f mpegts "srt://localhost:9000?mode=caller&streamid=live/test"
```
