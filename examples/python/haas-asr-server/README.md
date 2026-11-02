# haas-asr-server

Meeting の書き起こし bot（`moq-meeting-transcriber`）が `?asr=haas-ja` で呼ぶ、[Haas-ja-v1.0](https://huggingface.co/third-intelligence/Haas-ja-v1.0) の音声認識サーバです。
Haas-ja は NVIDIA NeMo でしか動かず、vLLM では動かせません（Parakeet-TDT 系、616M パラメータ）。
そのため [haas-asr](https://github.com/third-intelligence/Haas-ASR) でモデルを読み込み、vLLM と同じ `/health` と `/v1/audio/transcriptions` で答えます。
bot は Qwen3-ASR と同じく、発話の区間ごとに 16 kHz の WAV を書き起こし API に送ります。設計は [Live transcription for the Meeting example](../../../docs/architecture/ai-bots/meeting-transcriber.md) にあります。

- モデルは 1 つのスレッドで動かします。同時に来たリクエストは順番に処理します。
- 30 秒を超える区間は、haas-asr の `transcribe_longform` で静かな箇所で区切って書き起こします。
- 日本語のモデルを 1 つだけ提供するので、リクエストの `model` と `language` は見ません。
- Triton と CUDA が必要です。動かせるのは Linux と NVIDIA GPU の組み合わせだけです。

## 手元の GPU マシンで動かす

```shell
cd examples/python/haas-asr-server
uv run haas-asr-server --host 0.0.0.0 --port 8080
```

初回は Hugging Face からモデル（約 1.2 GB）をダウンロードします。
モデルを読み込むまで待ち受けを始めないので、`/health` が答えた時点で書き起こせます。
bot には `--asr haas-ja=http://<host>:8080` を渡します。

## Cloud Run にデプロイする

[音声認識を Cloud Run の GPU にデプロイする](../asr-on-cloud-run.md)の Qwen3-ASR などと同じく、L4 1 台の IAM 認証必須のサービス `moqt-haas-ja` にします。
重みは 1.2 GB と小さいので、GCS には置かず、リビジョンを固定してイメージに入れます。

> この手順はまだ GPU 上で通していません。初回のデプロイで確かめてください。

```shell
export PROJECT=your-project
export REGION=asia-southeast1
export SA=moqt-haas-ja@${PROJECT}.iam.gserviceaccount.com
export IMAGE=${REGION}-docker.pkg.dev/${PROJECT}/haas-asr/haas-asr-server:v1.0
```

### 1. イメージをビルドする

リポジトリのルートで実行します。

```shell
gcloud artifacts repositories create haas-asr --project=${PROJECT} --location=${REGION} --repository-format=docker
gcloud builds submit --project=${PROJECT} --region=${REGION} --tag=${IMAGE} \
  --machine-type=e2-highcpu-8 --disk-size=100 --timeout=3600s examples/python/haas-asr-server
```

### 2. サービスアカウントを作り、デプロイする

```shell
gcloud iam service-accounts create moqt-haas-ja --project=${PROJECT}
gcloud beta run deploy moqt-haas-ja \
  --project=${PROJECT} --region=${REGION} --image=${IMAGE} \
  --service-account=${SA} --no-allow-unauthenticated \
  --gpu=1 --gpu-type=nvidia-l4 --no-gpu-zonal-redundancy \
  --cpu=8 --memory=32Gi --no-cpu-throttling \
  --concurrency=8 --min-instances=0 --max-instances=1 --port=8080 --timeout=600 \
  --startup-probe=httpGet.path=/health,httpGet.port=8080,initialDelaySeconds=5,periodSeconds=10,timeoutSeconds=5,failureThreshold=36
```

### 3. 書き起こし bot から呼ぶ

```shell
gcloud run services add-iam-policy-binding moqt-haas-ja --project=${PROJECT} --region=${REGION} \
  --member=serviceAccount:<bot のサービスアカウント> --role=roles/run.invoker
ASR_URL=$(gcloud run services describe moqt-haas-ja --project=${PROJECT} --region=${REGION} --format 'value(status.url)')
uv run python -m moq_meeting_transcriber.bot --asr haas-ja=${ASR_URL} --metadata-auth
```

`--asr` は他の GPU のサービスと並べて指定できます。手元の bot から呼ぶときは `--metadata-auth` の代わりに `--gcloud-auth` を付けます。

## テスト

```shell
uv run pytest
```

テストは HTTP の処理だけを、モデルの代わりの偽物で確かめます。GPU は要りません。

## ライセンス

モデル `third-intelligence/Haas-ja-v1.0` と haas-asr は Apache License 2.0 です。このリポジトリはそれらを含めず、ビルド時に取得します。
