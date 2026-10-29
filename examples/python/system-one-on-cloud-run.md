# d1-2B を Cloud Run にデプロイする

MoQ Chat Moderation（`moq-chat-moderation`）、MoQ Camera Detection（`moq-camera-detection`）と MoQ PTZ Tracking（`moq-ptz-tracking`）の bot が呼ぶ判定サーバです。
どの bot も、1 つの Cloud Run サービス `moqt-system-one` で動く決定モデル [d1-2B](https://huggingface.co/LiquidAI/d1-2B) に、System One の `/v1/systemone` で質問します。

| bot | 用途 | 質問 |
| --- | --- | --- |
| MoQ Chat Moderation | チャットのラベル判定（テキスト） | `noul`（ラベルごとに当てはまる確率） |
| MoQ Camera Detection | カメラ映像への質問（画像） | `choice`（ページが送った選択肢） |
| MoQ PTZ Tracking | 追従する対象の枠（画像） | 対象ごとの `boxes`（当てはまるものすべての枠） |

d1-2B のリポジトリには HTTP サーバが含まれていません。そのため、このリポジトリの [`system-one-server`](system-one-server) で配信します。

| 構成 | 理由 |
| --- | --- |
| IAM 認証必須。専用のサービスアカウントを使う | GPU の費用がかかるため |
| Private Google Access を有効にした専用サブネット | `default` サブネットの設定を変えずに、GCS から重みを速く読むため |
| 重みを読み取り専用でマウントし、起動時に `/dev/shm` へコピーする | GCS のマウントから直接読み込むと遅いため |
| コピーを `timeout -k 10 240` で打ち切り、スタートアッププローブは 36 回（約 6 分）まで | GCS の障害で起動が止まったまま課金され続けないようにするため |

> この手順は、d3-flash の手順をもとに書いたもので、まだ GPU 上で通していません。初回のデプロイで確かめてください。

## 前提

- Cloud Run・Cloud Build・Artifact Registry・Cloud Storage・Compute Engine の API が有効になっていること
- L4 を使えるリージョン（例: `asia-southeast1`）で GPU の割り当てがあること

```shell
export PROJECT=your-project
export REGION=asia-southeast1
export BUCKET=your-system-one-weights
export SA=system-one@${PROJECT}.iam.gserviceaccount.com
export D1_REVISION=cfe5760ff5d5998f91d9ac56bd4b2dbd96996d41
export IMAGE=${REGION}-docker.pkg.dev/${PROJECT}/system-one/system-one-server:torch2.13-cu130
```

`D1_REVISION` は `system-one-server` の `DEFAULT_REVISION` と同じ commit にします。d1-2B はリポジトリのコードを実行して読み込む（`trust_remote_code`）ため、確かめた commit に固定します。

## 1. 重みを GCS に置き、イメージをビルドする

重み（約 3.8 GB）のダウンロードも Cloud Build の中で行います。リポジトリのルートで実行します。

```shell
gcloud storage buckets create gs://${BUCKET} --project=${PROJECT} --location=${REGION} \
  --uniform-bucket-level-access --public-access-prevention
gcloud artifacts repositories create system-one --project=${PROJECT} --location=${REGION} --repository-format=docker
cat > /tmp/system-one-cloudbuild.yaml <<EOF
steps:
  - name: python:3.12-slim
    entrypoint: bash
    args:
      - -c
      - >-
        pip install -q "huggingface_hub>=1.5,<2" &&
        hf download LiquidAI/d1-2B --revision ${D1_REVISION} --local-dir /workspace/d1-2B &&
        rm -rf /workspace/d1-2B/.cache
  - name: gcr.io/google.com/cloudsdktool/cloud-sdk:slim
    args: ["gcloud", "storage", "cp", "-r", "/workspace/d1-2B", "gs://${BUCKET}/"]
  - name: gcr.io/cloud-builders/docker
    args: ["build", "-t", "${IMAGE}", "."]
images: ["${IMAGE}"]
options:
  machineType: E2_HIGHCPU_8
timeout: 3600s
EOF
gcloud builds submit --project=${PROJECT} --region=${REGION} --config=/tmp/system-one-cloudbuild.yaml examples/python/system-one-server
```

## 2. サービスアカウントとサブネットを作る

```shell
gcloud iam service-accounts create system-one --project=${PROJECT}
gcloud storage buckets add-iam-policy-binding gs://${BUCKET} \
  --member=serviceAccount:${SA} --role=roles/storage.objectViewer
gcloud compute networks subnets create system-one --project=${PROJECT} --network=default --region=${REGION} \
  --range=10.40.0.0/26 --enable-private-ip-google-access
```

`default` が自動モードのネットワークの場合、サブネットの範囲は `10.128.0.0/9` と重ならないようにします。

## 3. デプロイする

```shell
gcloud beta run deploy moqt-system-one \
  --project=${PROJECT} --region=${REGION} --image=${IMAGE} \
  --service-account=${SA} --no-allow-unauthenticated \
  --gpu=1 --gpu-type=nvidia-l4 --no-gpu-zonal-redundancy \
  --cpu=4 --memory=16Gi --no-cpu-throttling \
  --concurrency=8 --min-instances=0 --max-instances=1 --port=8080 --timeout=300 \
  --network=default --subnet=system-one --vpc-egress=all-traffic \
  --set-env-vars=HF_HUB_OFFLINE=1 \
  --add-volume=name=weights,type=cloud-storage,bucket=${BUCKET},readonly=true,mount-options=enable-buffered-read=true \
  --add-volume-mount=volume=weights,mount-path=/mnt/gcs \
  --startup-probe=httpGet.path=/health,httpGet.port=8080,initialDelaySeconds=5,periodSeconds=10,timeoutSeconds=5,failureThreshold=36 \
  --command=/bin/bash \
  --args="-c","timeout -k 10 240 cp -r /mnt/gcs/d1-2B /dev/shm/d1-2B || exit 1; exec system-one-server --model /dev/shm/d1-2B --host 0.0.0.0 --port 8080"
```

GPU を付けた Cloud Run の最小構成は 4 vCPU・16 GiB です。d1-2B の重みは bf16 で約 3.8 GB なので、L4（24 GB）に収まります。

## 4. bot から呼ぶ

プロジェクトのオーナー以外が bot を起動する場合は、そのアカウントに `roles/run.invoker` を付けます。

```shell
gcloud run services add-iam-policy-binding moqt-system-one --project=${PROJECT} --region=${REGION} \
  --member=user:someone@example.com --role=roles/run.invoker
```

手元の bot には `--gcloud-auth` を付け、`gcloud auth print-identity-token` の ID トークンで呼ばせます。Cloud Run や GCE で動かす bot には `--metadata-auth` を付け、サービスアカウントの ID トークンをメタデータサーバーから取らせます。起動方法は各 bot の README を参照してください。

手元で試すだけなら、`system-one-server` を手元で起動して `--system-one-url http://127.0.0.1:8000/v1/systemone` を渡します（[system-one-server](system-one-server/README.md)）。

## 費用と起動時間

- インスタンスが起きている間だけ課金されます。最後のリクエストから約 10 分で 0 台に戻ります。
- `--max-instances=1` なので、リクエストが増えても 1 時間あたりの費用は L4 1 台分を超えません。bot を動かし続けると、インスタンスも起き続けます。
- 0 台からの最初のリクエストにかかる時間は、まだ測っていません。

## ライセンス

- モデル `LiquidAI/d1-2B`（`LiquidAI/LFM3-VL-2B` ベース）は LFM Open License v1.0 です。このリポジトリは重みを含めず、ビルド時に Hugging Face から取得します。
