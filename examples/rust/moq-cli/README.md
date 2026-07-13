# moq-cli

エンコード済みの映像を MoQ relay に publish / subscribe する CLI。

## 使い方

```sh
# 配信
ffmpeg ... | moq-cli publish --relay <relay> --track <track> --codec <codec>

# 購読
moq-cli subscribe --relay <relay> --track <track> | ffplay -
```

## オプション

| フラグ | publish | subscribe | 説明 |
|---|:-:|:-:|---|
| `--relay` | ● | ● | 接続先 relay。例 `moqt://localhost:4433` |
| `--track` | ● | ● | full track name。例 `tokyo/cam01/video` |
| `--codec` | ● | | `--container loc` のとき必須。`avc3`。cmaf では不要 |
| `--container` | ● | | `loc`（既定）か `cmaf` |
| `--insecure` | ● | ● | 証明書検証を無効化。自己署名 relay 用 |

subscribe は codec と container を catalog から読むので指定しない。timestamp は wall-clock で自動付与。入力・出力は stdin/stdout 固定。

## コンテナ

publish する側が、視聴側の再生方法に合わせて `--container` を選ぶ。

| container | 入力 | moq-cli の仕事 | codec 知識 |
|---|---|---|---|
| `loc`（既定） | 生 elementary stream（annex-b） | フレーム分割・keyframe 判定・1frame=1object | 必要 |
| `cmaf` | CMAF（moof+mdat, ffmpeg `-f mp4 -movflags frag...`） | 箱境界で Object 化するだけ | 不要 |

## 例

```sh
# ローカル loopback
cat sample.h264 | moq-cli publish --relay moqt://localhost:4433 --track live/video --codec avc3 --insecure
moq-cli subscribe --relay moqt://localhost:4433 --track live/video --insecure | ffplay -

# Mac カメラ
ffmpeg -f avfoundation -framerate 30 -video_size 1280x720 -pix_fmt nv12 -i "0" \
  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 30 -f h264 - \
  | moq-cli publish --relay moqt://localhost:4433 --track live/video --codec avc3 --insecure

# mp4 から（ffmpeg で annex-b にして渡す）
ffmpeg -i movie.mp4 -c:v copy -bsf:v h264_mp4toannexb -f h264 - \
  | moq-cli publish --relay moqt://localhost:4433 --track live/video --codec avc3 --insecure
```

## Raspberry Pi

Mac から cargo-zigbuild で静的バイナリを作って scp する。

```sh
cargo zigbuild --release --target aarch64-unknown-linux-musl -p moq-cli
scp target/aarch64-unknown-linux-musl/release/moq-cli pi@pi-cam.local:~/

# ライブカメラ
rpicam-vid -t 0 --codec h264 --inline --width 1280 --height 720 --framerate 30 -o - \
  | ./moq-cli publish --relay moqt://<relay>:443 --track live/video --codec avc3
```

## ビルド・テスト

```sh
cargo build --release -p moq-cli
cargo test -p moq-cli
```

## 設計メモ

- **catalog 取得**: 後から join した subscriber に届けるため publisher が各キーフレーム（group 境界）で catalog を再送している。正しい形は「一度だけ publish、subscriber は joining fetch（`Subscriber::fetch_relative_joining`）で取得」。
- **LOC payload**: payload=生 annex-b。Object Header Extension に Capture Timestamp を 0xB（ImmutableExtensions）で `"loc:"+JSON(packages::loc::LocHeader)` として入れている 。spec §2.3.1.1 は本来 Capture Timestamp を ID=2 の bare varint で送る。moqt crate の `ExtensionHeaders` が3種（`0x3c`/`0x3e`/`0xb`）ハードコードで任意 ID を送れないため 0xB 相乗り。
- spec: `spec/draft-ietf-moq-loc-01.txt`(LOC) / `draft-ietf-moq-msf-00.txt`(MSF) / `draft-ietf-moq-transport-14.txt`(MoQT)

## 実装状況

- [x] publish / subscribe（H.264/avc3 の LOC 配信）
- [x] ブラウザ再生（WebCodecs）・Pi 実機配信
- [x] catalog の publish / subscribe（subscribe で取得。publisher は各キーフレームで再送する暫定実装）
- [x] CLI 整理（`--container` 追加、`--fps`/`--input` 撤廃、timestamp を wall-clock 化）
- [ ] catalog を joining fetch で取得し、publisher は catalog を一度だけ publish
- [ ] VP8 / VP9（LOC 経路の codec splitter）
- [ ] `--container cmaf`（CMAF パススルー・MSE 再生）
- [x] payload JSON 廃止 → 生 annex-b ＋ Capture Timestamp を 0xB 拡張ヘッダに（browser 互換の `"loc:"+JSON` 相乗り）
- [ ] spec-true LOC 化（Capture Timestamp を Object Header Extension ID=2 varint で送る。moqt の任意 ID 拡張対応が前提）
