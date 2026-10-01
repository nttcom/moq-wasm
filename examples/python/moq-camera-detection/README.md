# moq-camera-detection

ブラウザの MoQ Camera Detection ページ（`examples/browser/examples/moq-camera-detection`）が H.264 で配信したカメラ映像を
pipecat の bot が受け取り、Cloud Run 上の djev-vision（[djev-run](https://github.com/taeold/djev-run) の画像対応版）で
ページが送った質問に選択肢から答えます。答えは MSF の event timeline として publish され、ページに表示されます。

```mermaid
sequenceDiagram
    participant B as Browser
    participant R as Relay
    participant P as pipecat bot
    participant D as djev-vision (Cloud Run)
    B->>R: prompt (anon/moq-camera-detection/camera)、{"question": "...", "choices": ["はい", "いいえ"]}
    B->>R: video (anon/moq-camera-detection/camera)、1 キーフレーム 1 group
    R->>P: 全オブジェクト（キーフレーム + 差分フレーム）
    P->>P: PyAV で H.264 をデコードし、0.1 秒ごとに 1 枚 JPEG に
    P->>D: /v1/chat/completions 画像 + 質問 + 番号付きの選択肢
    D-->>P: 1
    P->>R: eventtimeline (anon/moq-camera-detection/detector)
    R->>B: {"l": [groupId, objectId], "data": {"prompt": [promptGroupId, 0], "answer": "はい"}}
```

## djev-vision

djev-run を `--language-model-only` なしで起動し、画像を受け付けるようにした Cloud Run サービス `djev-vision`（asia-southeast1）です。
vLLM の画像前処理キャッシュは API とエンジンのプロセス間でずれるため、`--mm-processor-cache-gb 0` で無効にしています。
IAM 認証が必須なので、bot を起動するアカウントには `roles/run.invoker` が必要です。停止後の最初の判定はコールドスタートで約 3 分かかります。

vLLM は拡散モデルの出力を選択肢に縛れない（structured outputs 非対応）ため、選択肢に番号を振って番号だけで答えるよう指示し、
返答の最終行の先頭の数字を読みます。選択肢の番号でなければ「判定できません」と表示します。

## 起動

各コマンドを別々のターミナルで実行します。

1. relay を起動する（Cloud relay を使う場合は不要）

   ```shell
   make relay
   ```

2. bot を起動する

   ```shell
   cd examples/python/moq-camera-detection
   uv run python -m moq_camera_detection.bot --insecure \
     --djev-url "$(gcloud run services describe djev-vision --region asia-southeast1 --format 'value(status.url)')/v1/chat/completions" \
     --gcloud-auth
   ```

   Cloud relay を使う場合は `--insecure` の代わりに `--relay-url https://relay-1.moqt.research.skyway.io:443` を渡します。

3. ページを開く

   ```shell
   make browser
   make chrome
   ```

   ハブから MoQ Camera Detection を開き、bot と同じ relay を選んで Join します（既定は Cloud relay-1）。Cloud relay だけを使う場合、
   `make chrome` は不要です。

## トラック

| namespace | track | 送信元 | 内容 |
| --- | --- | --- | --- |
| `anon/moq-camera-detection/camera` | `prompt` | ブラウザ | `{"question": "...", "choices": [...]}`。適用するたびに 1 件 1 group で送る。質問は 200 文字以内、選択肢は 2〜9 個で各 40 文字以内 |
| `anon/moq-camera-detection/camera` | `video` | ブラウザ | H.264 Annex B（640x480、ソフトウェアエンコード）。キーフレームごとに group を始める |
| `anon/moq-camera-detection/detector` | `eventtimeline` | bot | draft-ietf-moq-msf-01 §8 の event timeline。`l` で判定したフレーム、`data.prompt` で使った prompt の位置を指す |

- bot は group ごとにデコーダを作って全フレームをデコードし、0.1 秒ごとに 1 枚を判定に回します。判定中のリクエストが 8 件に
  達している間の画像と、デコードから 2 秒以上たった画像は捨てるので、コールドスタート中にたまったフレームは判定されません。
- 判定は並列に走るため、新しいフレームの判定より後に返ってきた古いフレームの判定は publish しません。
- ページは最後に送った prompt の答えだけを表示します。prompt は `anon/` 配下なので、同じ relay の誰でも送れます。
- ブラウザはハードウェアエンコーダを使いません。macOS のハードウェアエンコーダは、偽カメラのファイル入力など一部のフレームで
  数フレーム出力したあと止まるためです。

## テスト

```shell
uv run pytest
```

E2E はリポジトリのルートで実行します。relay・VTS・vite・偽の判定サーバ・bot を起動し、Playwright の偽カメラでページを操作します。

```shell
node scripts/run-camera-detection-e2e.mjs
```
