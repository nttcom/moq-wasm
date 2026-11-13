# haas-asr-server

Meeting の書き起こし bot（`moq-meeting-transcriber`）が `?asr=haas-ja` で呼ぶ、[Haas-ja-v1.0](https://huggingface.co/third-intelligence/Haas-ja-v1.0) の音声認識サーバです。
Haas-ja は NVIDIA NeMo でしか動かないので、[haas-asr](https://github.com/third-intelligence/Haas-ASR) でモデルを読み込み、vLLM と同じ `/health` と `/v1/audio/transcriptions` で答えます。
設計は [Live transcription for the Meeting example](../../../docs/architecture/ai-bots/meeting-transcriber.md) にあります。

- リクエストは 1 件ずつ順番に処理します。リクエストの `model` と `language` は見ません。
- Linux と NVIDIA GPU（Triton、CUDA）が必要です。

## 手元の GPU マシンで動かす

```shell
cd examples/python/haas-asr-server
uv run --extra model haas-asr-server --host 0.0.0.0 --port 8080
```

初回はモデル（約 1.2 GB）をダウンロードします。`/health` が答えたら書き起こせます。
bot には `--asr haas-ja=http://<host>:8080` を渡します。

## Cloud Run にデプロイする

[音声認識を Cloud Run の GPU にデプロイする](../asr-on-cloud-run.md)の他のモデルと同じく、L4 1 台の IAM 認証必須のサービス `moqt-haas-ja` にします。重みはイメージに入れます。

> この手順はまだ GPU 上で通していません。初回のデプロイで確かめてください。

```shell
export PROJECT=your-project
export REGION=asia-southeast1
export SA=moqt-haas-ja@${PROJECT}.iam.gserviceaccount.com
export IMAGE=${REGION}-docker.pkg.dev/${PROJECT}/haas-asr/haas-asr-server:v1.0

# 1. イメージをビルドする（リポジトリのルートで）
gcloud artifacts repositories create haas-asr --project=${PROJECT} --location=${REGION} --repository-format=docker
gcloud builds submit --project=${PROJECT} --region=${REGION} --tag=${IMAGE} \
  --machine-type=e2-highcpu-8 --disk-size=100 --timeout=3600s examples/python/haas-asr-server

# 2. サービスアカウントを作り、デプロイする
gcloud iam service-accounts create moqt-haas-ja --project=${PROJECT}
gcloud beta run deploy moqt-haas-ja \
  --project=${PROJECT} --region=${REGION} --image=${IMAGE} \
  --service-account=${SA} --no-allow-unauthenticated \
  --gpu=1 --gpu-type=nvidia-l4 --no-gpu-zonal-redundancy \
  --cpu=8 --memory=32Gi --no-cpu-throttling \
  --concurrency=8 --min-instances=0 --max-instances=1 --port=8080 --timeout=600 \
  --startup-probe=httpGet.path=/health,httpGet.port=8080,initialDelaySeconds=5,periodSeconds=10,timeoutSeconds=5,failureThreshold=36

# 3. 書き起こし bot から呼ぶ
gcloud run services add-iam-policy-binding moqt-haas-ja --project=${PROJECT} --region=${REGION} \
  --member=serviceAccount:<bot のサービスアカウント> --role=roles/run.invoker
ASR_URL=$(gcloud run services describe moqt-haas-ja --project=${PROJECT} --region=${REGION} --format 'value(status.url)')
uv run python -m moq_meeting_transcriber.bot --asr haas-ja=${ASR_URL} --metadata-auth
```

手元の bot から呼ぶときは `--metadata-auth` の代わりに `--gcloud-auth` を付けます。

## テスト

```shell
uv run pytest
```

モデルの代わりの偽物で HTTP の処理だけを確かめるので、GPU は要りません。

## ライセンス

モデル `third-intelligence/Haas-ja-v1.0` と haas-asr は Apache License 2.0 です。このリポジトリはそれらを含めず、ビルド時に取得します。
