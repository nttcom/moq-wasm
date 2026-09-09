# live-viewer

`bridges/live-ingest` が RTMP / SRT から MoQT へ流した配信を視聴します。catalog に載っている映像
track を切り替えられるので、`--transcode` で生成した下位画質（`video_480p` / `video_360p`）の確認にも
使えます。

## 使い方

```shell
make relay
make live-ingest            # LIVE_INGEST_TRANSCODE=1 で下位画質も配信する
make ffmpeg-rtmp            # または make ffmpeg-srt
make browser
make chrome                 # 自己署名証明書の relay に接続するため
```

Chrome で `examples/live-viewer/index.html` を開き、relay URL と namespace（RTMP は `app/stream`、
SRT は stream ID）を入れて Watch を押します。`?moqtUrl=...&trackNamespace=...` でも指定できます。

## ペイロード形式

live-ingest は各 object を `[meta_len (u32 BE)][meta JSON][coded data]` で送り、ブラウザ publisher は
coded data のみを送って metadata を MoQT の LOC 拡張ヘッダに載せます。`utils/media/ingestChunk.ts` が
両方を受け付け、前者は JSON を外して capture timestamp を LOC ヘッダ相当に変換します。

## E2E

relay・live-ingest・配信元・dev server が動いている状態で実行します。

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=live \
npm --prefix examples/browser run e2e:live-viewer
```
