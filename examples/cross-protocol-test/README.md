# Cross-Protocol Test

QUIC と WebTransport を跨いだ映像の Pub/Sub が動作することを手動で確認するためのアプリケーション。

## 構成

```
                              [Relay]
                          (dual protocol)
[QUIC Publisher]           port 4433               [Browser Subscriber]
 Rust ネイティブ            (QUIC + WT)             moqtail (WASM) で接続
 カメラ映像                      ↓                   fMP4 を MSE で再生
   ↓                       両プロトコル間で
 ffmpeg (子プロセス)        データをリレー           [QUIC Subscriber]
   ↓                                                 Rust ネイティブ
 fMP4 → MoQT 送信                                    fMP4 を stdout に出力
```

QUIC → QUIC、および QUIC → WebTransport の両方で検証する。

## データフォーマット

- コンテナ: fMP4 (fragmented MP4)
- 映像コーデック: H.264 (yuv420p, MSE 互換)
- 音声: なし（映像のみ）

### MoQT データモデルへのマッピング

> **注意**: この実装は MoQT のカタログ仕様 (draft-ietf-moq-catalogformat) に準拠していない。
> 標準的な CMAF over MoQT では、init segment はカタログの `initData` フィールドで配信するが、
> この実装ではカタログを使用せず、各 Group の Object 0 に init segment を独自にマッピングしている。
> あくまでクロスプロトコル中継の動作検証を目的とした簡易的な実装である。

```
Track: video
  Group N (= 1 GoP):
    Subgroup 0:
      Object 0: init segment (ftyp+moov)
      Object 1: media segment (moof+mdat)
```

- 1 Group = 1 Subgroup = 1 GoP（1 秒間隔）
- Object 0 に init segment (ftyp+moov) を毎回送る（途中参加対応）
- Object 1 に media segment (moof+mdat) を送る

## 前提条件

- macOS（avfoundation によるカメラキャプチャ）
- ffmpeg / ffplay がインストール済み
- Node.js（browser-subscriber 用）
- Relay サーバーが dual protocol (QUIC + WebTransport) で起動できること

## 使い方

### 1. Relay を起動

```sh
cargo run -p relay
```

### 2. Publisher を起動

```sh
cargo run -p quic-publisher
```

ログに `using namespace namespace="live-XXXX"` と表示される。
Publisher は subscriber が接続するまで待機し、接続後に ffmpeg を起動して映像送信を開始する。

### 3a. Browser Subscriber（推奨）

```sh
cd examples/cross-protocol-test/browser-subscriber
npm install
npm run dev
```

ブラウザで http://localhost:5173/ を開き、namespace を入力して Subscribe ボタンを押す。

### 3b. QUIC Subscriber

ffplay にパイプしてリアルタイム再生:

```sh
cargo run -p quic-subscriber -- <namespace> video 2>/dev/null | ffplay -
```

## QUIC Publisher

ffmpeg を子プロセスとして起動し、カメラキャプチャ・エンコード・fMP4 muxing を委譲する。
Rust 側は ffmpeg の stdout から fMP4 バイト列を読み、box 単位でパースして MoQT で送信する。

subscriber が接続するまで ffmpeg は起動しない。これにより不要なバッファリングを防ぐ。

### ffmpeg コマンド

```sh
ffmpeg \
  -f avfoundation -framerate 30 -video_size 640x480 -i "0:none" \
  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 30 \
  -f mp4 -movflags frag_keyframe+empty_moov+default_base_moof \
  pipe:1
```

- `-pix_fmt yuv420p`: MSE が対応するピクセルフォーマット
- `-g 30`: 30fps で 1 秒ごとのキーフレーム
- `default_base_moof`: MSE が要求する movie-fragment-relative-addressing

### 処理フロー

1. Relay に QUIC で接続し namespace を publish
2. subscriber の接続を待つ
3. subscriber が来たら ffmpeg を子プロセスで起動
4. stdout から fMP4 の box を読み出す（先頭 8 バイト: 4 バイト size + 4 バイト type）
5. `ftyp` + `moov` box → init segment として保持
6. `moof` + `mdat` box のペア → media segment として MoQT Object に詰めて送信

## Browser Subscriber

moqtail (WASM) を使ったブラウザアプリ。WebTransport で Relay に接続し、受信した fMP4 を MSE (Media Source Extensions) でリアルタイム再生する。

### 処理フロー

1. moqtail で Relay (port 4433) に WebTransport 接続
2. MoQT セッション確立 (SETUP)
3. Subscribe（namespace と track name を UI から指定）
4. init segment から avcC を解析してコーデック文字列を自動検出
5. MSE の MediaSource + SourceBuffer を初期化
6. 受信した Object からデータを取り出す:
   - Object 0 (init segment): 初回のみ SourceBuffer に追加
   - Object 1 (media segment): SourceBuffer に追加
7. バッファが 2 秒以上溜まると自動で live edge にシーク

### 技術的な注意点

- relay の自己署名証明書のハッシュを vite ビルド時に注入し、WebTransport の `serverCertificateHashes` で指定
- relay の証明書を再生成した場合は vite の再起動が必要

## QUIC Subscriber

Rust ネイティブ CLI として実装。QUIC で Relay に接続し、受信した fMP4 を stdout に書き出す。
ffplay にパイプしてリアルタイム再生、またはファイルに保存して確認する用途向け。

### 処理フロー

1. QUIC で Relay (port 4433) に接続
2. MoQT セッション確立 (SETUP)
3. Subscribe（namespace と track_name をコマンドライン引数で指定）
4. 受信した Object からデータを取り出す:
   - Object 0 (init segment): 初回のみ stdout に書き出す（2 回目以降はスキップ）
   - Object 1 (media segment): そのまま stdout に書き出す

### 設計上の注意点

- tracing の出力先は stderr（stdout は fMP4 データ用）
- init segment は初回 group でのみ書き出す。毎回書き出すと fMP4 ストリームとして壊れる（duplicated MOOV Atom）
- 証明書検証はスキップしている（`verify_certificate: false`）
- `ffplay -` にパイプしてリアルタイム再生が可能
