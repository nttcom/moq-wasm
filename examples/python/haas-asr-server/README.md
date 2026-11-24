# haas-asr-server

`moq-meeting-transcriber` が `asr: haas-ja` で呼び出す [Haas-ja-v1.0](https://huggingface.co/third-intelligence/Haas-ja-v1.0) 用 ASR サーバです（設計: [meeting-transcriber.md](../../../docs/architecture/ai-bots/meeting-transcriber.md)）。
NVIDIA NeMo 依存のため [haas-asr](https://github.com/third-intelligence/Haas-ASR) でモデルを読み込み、vLLM 互換の `/health` と `/v1/audio/transcriptions` を提供します。

- 推論は 1 件ずつ直列実行（リクエストの `model`・`language` は無視）。
- Linux + NVIDIA GPU（Triton / CUDA）が必須。

## ローカル GPU で起動

```shell
cd examples/python/haas-asr-server
uv run --extra model haas-asr-server --host 0.0.0.0 --port 8080
```

初回はモデル（約 1.2 GB）を取得します。`/health` 応答後、bot に `--asr haas-ja=http://<host>:8080` を渡します。

## Cloud Run にデプロイ

[asr-on-cloud-run.md](../asr-on-cloud-run.md) と同様、L4 1 基・IAM 認証必須の `moqt-haas-ja` としてデプロイします（重みはイメージ同梱）。

> 未検証（GPU 環境での初回デプロイ時に要確認）。

```shell
export PROJECT=your-project
export REGION=asia-southeast1
export SA=moqt-haas-ja@${PROJECT}.iam.gserviceaccount.com
export IMAGE=${REGION}-docker.pkg.dev/${PROJECT}/haas-asr/haas-asr-server:v1.0

# 1. イメージビルド（リポジトリルートで実行）
gcloud artifacts repositories create haas-asr --project=${PROJECT} --location=${REGION} --repository-format=docker
gcloud builds submit --project=${PROJECT} --region=${REGION} --tag=${IMAGE} \
  --machine-type=e2-highcpu-8 --disk-size=100 --timeout=3600s examples/python/haas-asr-server

# 2. サービスアカウント作成とデプロイ
gcloud iam service-accounts create moqt-haas-ja --project=${PROJECT}
gcloud beta run deploy moqt-haas-ja \
  --project=${PROJECT} --region=${REGION} --image=${IMAGE} \
  --service-account=${SA} --no-allow-unauthenticated \
  --gpu=1 --gpu-type=nvidia-l4 --no-gpu-zonal-redundancy \
  --cpu=8 --memory=32Gi --no-cpu-throttling \
  --concurrency=8 --min-instances=0 --max-instances=1 --port=8080 --timeout=600 \
  --startup-probe=httpGet.path=/health,httpGet.port=8080,initialDelaySeconds=5,periodSeconds=10,timeoutSeconds=5,failureThreshold=36

# 3. transcriber bot からの呼び出し許可
gcloud run services add-iam-policy-binding moqt-haas-ja --project=${PROJECT} --region=${REGION} \
  --member=serviceAccount:<bot のサービスアカウント> --role=roles/run.invoker
ASR_URL=$(gcloud run services describe moqt-haas-ja --project=${PROJECT} --region=${REGION} --format 'value(status.url)')
```

`BOT_DISPATCHER_TYPES` の `transcriber` に `--asr haas-ja=${ASR_URL}` と `--metadata-auth`（ローカル実行時は `--gcloud-auth`）を追加します（[moq-meeting-transcriber](../moq-meeting-transcriber/README.md)）。

## テスト

```shell
uv run pytest   # モックモデルで HTTP 層のみ検証（GPU 不要）
```

## ライセンス

`third-intelligence/Haas-ja-v1.0` および `haas-asr` は Apache License 2.0 です（本リポジトリには含めずビルド時に取得）。
