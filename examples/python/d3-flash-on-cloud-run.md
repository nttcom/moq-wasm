# d3-flash を Cloud Run にデプロイする

MoQ Chat Moderation（`moq-chat-moderation`）、MoQ Camera Detection（`moq-camera-detection`）と MoQ PTZ Tracking（`moq-ptz-tracking`）の bot が呼ぶ判定サーバです。
どれも 1 つの Cloud Run サービス `d3-flash` で動く決定モデル [d3-flash](https://huggingface.co/vllm-sr/d3-flash) に、System One の `/v1/systemone` で質問します。

| bot | 用途 | 質問 |
| --- | --- | --- |
| MoQ Chat Moderation | チャットの暴言判定（テキスト） | `noul`（暴言である確率） |
| MoQ Camera Detection | カメラ映像への質問（画像） | `choice`（ページが送った選択肢） |
| MoQ PTZ Tracking | 追従する対象の位置（画像） | `noul`（写っているか）、写っていれば `choice`（5×5 のマス目） |

サーバはモデルのリポジトリに含まれる `d3_server.py` です。イメージには PyTorch と依存パッケージだけを入れます。

| 構成 | 理由 |
| --- | --- |
| IAM 認証必須。専用のサービスアカウントを使う | GPU の費用がかかるため |
| Private Google Access を有効にした専用サブネット | `default` サブネットの設定を変えずに、GCS から重みを速く読むため |
| 重みを読み取り専用でマウントし、起動時に `/dev/shm` へコピーする | GCS のマウントから直接読み込むと遅いため |
| コピーを `timeout -k 10 240` で打ち切り、スタートアッププローブは 36 回（約 6 分）まで | GCS の障害で起動が止まったまま課金され続けないようにするため |

## 前提

- Cloud Run・Cloud Build・Artifact Registry・Cloud Storage・Compute Engine の API が有効になっていること
- L4 を使えるリージョン（例: `asia-southeast1`）で GPU の割り当てがあること

```shell
export PROJECT=your-project
export REGION=asia-southeast1
export BUCKET=your-d3-flash-weights
export SA=d3-flash@${PROJECT}.iam.gserviceaccount.com
export D3_FLASH_REVISION=f6075bff3c0f7d5704e353d0fb2ebf94888d1691
export IMAGE=${REGION}-docker.pkg.dev/${PROJECT}/d3-flash/d3-runtime:torch2.13-cu130
```

## 1. 重みを GCS に置き、イメージをビルドする

重み（約 17 GB）のダウンロードも Cloud Build の中で行います。

```shell
gcloud storage buckets create gs://${BUCKET} --project=${PROJECT} --location=${REGION} \
  --uniform-bucket-level-access --public-access-prevention
gcloud artifacts repositories create d3-flash --project=${PROJECT} --location=${REGION} --repository-format=docker
mkdir -p /tmp/d3-flash-build
cat > /tmp/d3-flash-build/Dockerfile <<'DOCKERFILE'
FROM pytorch/pytorch:2.13.0-cuda13.0-cudnn9-runtime
ENV PIP_BREAK_SYSTEM_PACKAGES=1
RUN apt-get update && apt-get install -y --no-install-recommends gcc && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir "transformers==5.17.0" "accelerate>=1.10" "safetensors>=0.8" "huggingface_hub>=1.5,<2" \
    "flash-linear-attention==0.5.2" einops "pillow>=10" opencv-python-headless
DOCKERFILE
cat > /tmp/d3-flash-build/cloudbuild.yaml <<EOF
steps:
  - name: python:3.12-slim
    entrypoint: bash
    args:
      - -c
      - >-
        pip install -q "huggingface_hub>=1.5,<2" &&
        hf download vllm-sr/d3-flash --revision ${D3_FLASH_REVISION} --local-dir /workspace/d3-flash &&
        rm -rf /workspace/d3-flash/.cache
  - name: gcr.io/google.com/cloudsdktool/cloud-sdk:slim
    args: ["gcloud", "storage", "cp", "-r", "/workspace/d3-flash", "gs://${BUCKET}/"]
  - name: gcr.io/cloud-builders/docker
    args: ["build", "-t", "${IMAGE}", "."]
images: ["${IMAGE}"]
options:
  machineType: E2_HIGHCPU_8
  diskSizeGb: 100
timeout: 3600s
EOF
gcloud builds submit --project=${PROJECT} --region=${REGION} --config=/tmp/d3-flash-build/cloudbuild.yaml /tmp/d3-flash-build
```

`pytorch/pytorch` のイメージは PEP 668 でシステムの Python への `pip install` を拒むため、`PIP_BREAK_SYSTEM_PACKAGES=1` を付けます。

## 2. サービスアカウントとサブネットを作る

```shell
gcloud iam service-accounts create d3-flash --project=${PROJECT}
gcloud storage buckets add-iam-policy-binding gs://${BUCKET} \
  --member=serviceAccount:${SA} --role=roles/storage.objectViewer
gcloud compute networks subnets create d3-flash --project=${PROJECT} --network=default --region=${REGION} \
  --range=10.40.0.0/26 --enable-private-ip-google-access
```

`default` が自動モードのネットワークの場合、サブネットの範囲は `10.128.0.0/9` と重ならないようにします。

## 3. デプロイする

```shell
gcloud beta run deploy d3-flash \
  --project=${PROJECT} --region=${REGION} --image=${IMAGE} \
  --service-account=${SA} --no-allow-unauthenticated \
  --gpu=1 --gpu-type=nvidia-l4 --no-gpu-zonal-redundancy \
  --cpu=8 --memory=32Gi --no-cpu-throttling \
  --concurrency=8 --min-instances=0 --max-instances=1 --port=8080 --timeout=300 \
  --network=default --subnet=d3-flash --vpc-egress=all-traffic \
  --set-env-vars=HF_HUB_OFFLINE=1 \
  --add-volume=name=weights,type=cloud-storage,bucket=${BUCKET},readonly=true,mount-options=enable-buffered-read=true \
  --add-volume-mount=volume=weights,mount-path=/mnt/gcs \
  --startup-probe=httpGet.path=/health,httpGet.port=8080,initialDelaySeconds=5,periodSeconds=10,timeoutSeconds=5,failureThreshold=36 \
  --command=/bin/bash \
  --args="-c","timeout -k 10 240 cp -r /mnt/gcs/d3-flash /dev/shm/d3-flash || exit 1; exec python /dev/shm/d3-flash/d3_server.py --model /dev/shm/d3-flash --batch-size 3 --host 0.0.0.0 --port 8080"
```

## 4. bot から呼ぶ

プロジェクトのオーナー以外が bot を起動する場合は、そのアカウントに `roles/run.invoker` を付けます。

```shell
gcloud run services add-iam-policy-binding d3-flash --project=${PROJECT} --region=${REGION} \
  --member=user:someone@example.com --role=roles/run.invoker
```

手元の bot には `--gcloud-auth` を付け、`gcloud auth print-identity-token` の ID トークンで呼ばせます。GCE の VM で動かす bot には `--metadata-auth` を付け、VM のサービスアカウントの ID トークンをメタデータサーバーから取らせます。起動方法は各 bot の README を参照してください。

## 費用と起動時間

- インスタンスが起きている間だけ課金され、L4 1 台（8 vCPU・32 GiB）で約 $1.70/h です。最後のリクエストから約 10 分で 0 台に戻ります。
- 0 台からの最初のリクエストは、重みのコピーとモデルの読み込みで約 2 分 40 秒かかります。
- `--max-instances=1` なので、リクエストが増えても 1 時間あたりの費用は上の金額を超えません。bot を動かし続けると、インスタンスも起き続けます。

## ライセンス

- モデル `vllm-sr/d3-flash`（`Qwen/Qwen3.5-9B` ベース）と、同じリポジトリの `d3_server.py` は Apache License 2.0 です。このリポジトリはそれらを含めず、ビルド時に Hugging Face から取得します。
