# djev-run を Cloud Run にデプロイする

MoQ Chat Moderation（`moq-chat-moderation`）、MoQ Camera Detection（`moq-camera-detection`）と MoQ PTZ Tracking（`moq-ptz-tracking`）の bot が呼ぶ判定サーバです。
どれも [djev-run](https://github.com/taeold/djev-run) を Cloud Run の GPU で動かします。

| サービス | 用途 | 呼び出し先 |
| --- | --- | --- |
| `djev-dgemma` | チャットの暴言判定（テキスト、確率付き） | `/v1/systemone` |
| `djev-vision` | カメラ映像への質問（画像、選択肢の番号）・追従する対象の位置（画像、座標） | `/v1/chat/completions` |

基本の手順は djev-run の README の
[Deploy on Google Cloud Run](https://github.com/taeold/djev-run/tree/2f6e6b9b455eec27dcdc0a4cb71a3fafdec6acc8#deploy-on-google-cloud-run)
です。ここでは、その手順から変えた点と実際のコマンドを載せます。

| README の手順 | このリポジトリの構成 | 理由 |
| --- | --- | --- |
| `ghcr.io/taeold/djev-run` を使う | Cloud Build でソースからビルドし、Artifact Registry に置く | 2026-10-01 時点で、GHCR のイメージは Cloud Run で `Container import failed` になる |
| 認証なしで公開 | IAM 認証必須。専用のサービスアカウントを使う | GPU の費用がかかるため |
| `default` サブネット | Private Google Access を有効にした専用サブネット | `default` サブネットの設定を変えずに、GCS から重みを速く読む（VPC を通さないとコールドスタートに約 9 分かかる） |
| 重みのバケットを読み書き可能でマウント | 読み取り専用でマウント | 重みを書き換えないため |
| テキストのみ（`--language-model-only`） | `djev-vision` は画像エンコーダを読み込み、`--mm-processor-cache-gb 0` を付ける | 画像を受け付けるため。キャッシュを有効にすると画像リクエストが毎回 500 になる |

## 前提

- Cloud Run・Cloud Build・Artifact Registry・Cloud Storage・Compute Engine の API が有効になっていること
- RTX PRO 6000 を使えるリージョン（例: `asia-southeast1`）で GPU の割り当てがあること

```shell
export PROJECT=your-project
export REGION=asia-southeast1
export BUCKET=your-djev-weights
export SA=djev-run@${PROJECT}.iam.gserviceaccount.com
export DJEV_RUN_COMMIT=2f6e6b9b455eec27dcdc0a4cb71a3fafdec6acc8
export IMAGE=${REGION}-docker.pkg.dev/${PROJECT}/djev/djev-run:${DJEV_RUN_COMMIT:0:8}
```

## 1. モデルの重みを GCS に置く

djev-run の README の Step 1 と同じです。約 19 GB をダウンロードするので、手元の回線が遅い場合は Cloud Build の中で
`hf download` と `gcloud storage cp` を実行しても構いません。

```shell
gcloud storage buckets create gs://${BUCKET} --project=${PROJECT} --location=${REGION} \
  --uniform-bucket-level-access --public-access-prevention
hf download nvidia/diffusiongemma-26B-A4B-it-NVFP4 --local-dir /tmp/dgemma
gcloud storage cp -r /tmp/dgemma/* gs://${BUCKET}/dgemma/
```

## 2. イメージをビルドする

```shell
gcloud artifacts repositories create djev --project=${PROJECT} --location=${REGION} --repository-format=docker
cat > /tmp/djev-build.yaml <<EOF
steps:
  - name: gcr.io/cloud-builders/git
    entrypoint: bash
    args: ["-c", "git clone https://github.com/taeold/djev-run.git src && git -C src checkout ${DJEV_RUN_COMMIT}"]
  - name: gcr.io/cloud-builders/docker
    args: ["build", "-t", "${IMAGE}", "src"]
images: ["${IMAGE}"]
options:
  machineType: E2_HIGHCPU_8
  diskSizeGb: 100
timeout: 3600s
EOF
gcloud builds submit --project=${PROJECT} --region=${REGION} --no-source --config=/tmp/djev-build.yaml
```

## 3. サービスアカウントとサブネットを作る

```shell
gcloud iam service-accounts create djev-run --project=${PROJECT}
gcloud storage buckets add-iam-policy-binding gs://${BUCKET} \
  --member=serviceAccount:${SA} --role=roles/storage.objectViewer
gcloud compute networks subnets create djev-run --project=${PROJECT} --network=default --region=${REGION} \
  --range=10.40.0.0/26 --enable-private-ip-google-access
```

`default` が自動モードのネットワークの場合、サブネットの範囲は `10.128.0.0/9` と重ならないようにします。

## 4. デプロイする

2 つのサービスは同じイメージと同じリソースで動かします。

```shell
COMMON=(
  --project=${PROJECT} --region=${REGION} --image=${IMAGE}
  --service-account=${SA} --no-allow-unauthenticated
  --gpu=1 --gpu-type=nvidia-rtx-pro-6000 --no-gpu-zonal-redundancy
  --cpu=20 --memory=80Gi --no-cpu-throttling
  --concurrency=32 --min-instances=0 --max-instances=1 --port=8080
  --network=default --subnet=djev-run --vpc-egress=all-traffic
  --add-volume=name=weights,type=cloud-storage,bucket=${BUCKET},readonly=true,mount-options=enable-buffered-read=true
  --add-volume-mount=volume=weights,mount-path=/mnt/gcs
  --startup-probe=httpGet.path=/health,httpGet.port=8080,initialDelaySeconds=5,periodSeconds=10,timeoutSeconds=5,failureThreshold=60
)
```

`djev-dgemma` はイメージに組み込まれた `vllm serve` をそのまま使います。

```shell
gcloud beta run deploy djev-dgemma "${COMMON[@]}"
```

`djev-vision` は起動コマンドだけを差し替えます。

```shell
gcloud beta run deploy djev-vision "${COMMON[@]}" \
  --command=/bin/bash \
  --args="-c","export VLLM_ENABLE_V1_MULTIPROCESSING=0 VLLM_FLASHINFER_MOE_BACKEND=masked_gemm PYTHONPATH=/opt/dgemma && cp -r /mnt/gcs/dgemma /dev/shm/dgemma && exec vllm serve /dev/shm/dgemma --middleware server.SystemOneMiddleware --port 8080 --served-model-name djev-dgemma --allowed-origins '[\"*\"]' --trust-remote-code --enforce-eager --attention-backend TRITON_ATTN --kv-cache-memory 2G --max-num-seqs 32 --max-model-len 4096 --mm-processor-cache-gb 0 --diffusion-config '{\"canvas_length\":128}' --override-generation-config '{\"max_new_tokens\":null}'"
```

## 5. bot から呼ぶ

プロジェクトのオーナー以外が bot を起動する場合は、そのアカウントに `roles/run.invoker` をサービスごとに付けます。

```shell
gcloud run services add-iam-policy-binding djev-vision --project=${PROJECT} --region=${REGION} \
  --member=user:someone@example.com --role=roles/run.invoker
```

手元の bot には `--gcloud-auth` を付け、`gcloud auth print-identity-token` の ID トークンで呼ばせます。GCE の VM で動かす bot には `--metadata-auth` を付け、VM のサービスアカウントの ID トークンをメタデータサーバーから取らせます。起動方法は各 bot の README を参照してください。

## 費用と起動時間

- インスタンスが起きている間だけ課金され、RTX PRO 6000 1 台で約 $3.19/h です。最後のリクエストから約 10 分で 0 台に戻ります。
- 0 台からの最初のリクエストは、重みのコピーと vLLM の起動で約 2 分 40 秒〜3 分かかります。
- `--max-instances=1` なので、リクエストが増えても 1 時間あたりの費用は上の金額を超えません。bot を動かし続けると、インスタンスも起き続けます。

## ライセンス

- モデル `nvidia/diffusiongemma-26B-A4B-it-NVFP4` は Apache License 2.0 です。
- djev-run のリポジトリには、2026-10-01 時点でライセンスファイルがありません。このリポジトリは djev-run のコードを含めず、
  ビルド時に GitHub から取得します。
