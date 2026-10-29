# 音声認識を Cloud Run の GPU にデプロイする

Meeting の書き起こし bot（`moq-meeting-transcriber`）が `?asr=` で呼ぶ、GPU の音声認識サーバです。
どちらのモデルも vLLM で動かし、OpenAI 互換の API で答えます。
bot は発話の区間ごとに WAV を送ります。Qwen3-ASR には `/v1/audio/transcriptions` で、Granite Speech には `/v1/chat/completions` で送ります。
vLLM の書き起こし API は、Granite Speech に言語を書かない決まったプロンプトを渡すため、bot は日本語だと書いたプロンプトをチャット API で送ります。設計は [Live transcription for the Meeting example](../../docs/architecture/ai-bots/meeting-transcriber.md) にあります。

| `asr`（サービス名は `moqt-<asr>`） | モデル | vLLM の起動引数 |
| --- | --- | --- |
| `qwen3-asr` | [Qwen/Qwen3-ASR-1.7B](https://huggingface.co/Qwen/Qwen3-ASR-1.7B) | なし |
| `granite-speech-5-2b` | [ibm-granite/granite-speech-5.0-2b](https://huggingface.co/ibm-granite/granite-speech-5.0-2b) | `--max-model-len 2048` |

モデルごとに別のサービスにします。Qwen3-ASR のモデルカードは、そのモデルに合わせて固定した vLLM の nightly 版を求めているためです。
System One の判定サーバ（[d1-2B を Cloud Run にデプロイする](system-one-on-cloud-run.md)）とも別のサービスです。判定サーバは vLLM ではなく独自のサーバ（`system-one-server`）で動くためです。

| 構成 | 理由 |
| --- | --- |
| IAM 認証必須。専用のサービスアカウントを使う | GPU の費用がかかるため |
| 重みを GCS に置き、起動時に `/dev/shm` へコピーする | 判定サーバと同じく、GCS のマウントから直接読み込むと遅いため |
| モデル名を `--served-model-name <asr>` にする | bot は `?asr=` の名前をモデル名として送るため |

> この手順は、モデルカードと判定サーバ（当時は d3-flash）の手順をもとに書いたもので、まだ GPU 上で通していません。初回のデプロイで確かめてください。
> イメージは digest で固定します。Qwen3-ASR はモデルカードが求める nightly 版以降の `vllm/vllm-openai:nightly`、Granite Speech はリリース版の `vllm/vllm-openai` です。
> Granite Speech 2b のリポジトリには、`model.safetensors` のほかに `weight` と `bias` だけを持つ `out_llm.safetensors` があり、index ファイルはありません。vLLM は index のないディレクトリの safetensors をすべて読み込み、`out_llm.safetensors` の重みを置く先がなくて起動に失敗するため、ダウンロードから除きます。

## 前提

- Cloud Run・Cloud Build・Artifact Registry・Cloud Storage の API が有効になっていること
- L4 を使えるリージョン（例: `asia-southeast1`）で GPU の割り当てがあること

デプロイするモデルごとに、次の変数を決めます。

```shell
export PROJECT=your-project
export REGION=asia-southeast1
# qwen3-asr: Qwen/Qwen3-ASR-1.7B, no extra arguments.
# granite-speech-5-2b: ibm-granite/granite-speech-5.0-2b, --max-model-len 2048.
export ASR=qwen3-asr
export HF_MODEL=Qwen/Qwen3-ASR-1.7B
export VLLM_ARGS=
# Pin the Hugging Face revision and the vLLM image you tested.
export HF_REVISION=<commit of the model repository>
export VLLM_IMAGE=vllm/vllm-openai:<tag>@sha256:<digest>
export BUCKET=${PROJECT}-${ASR}-weights
export SA=moqt-${ASR}@${PROJECT}.iam.gserviceaccount.com
```

## 1. 重みを GCS に置く

```shell
gcloud storage buckets create gs://${BUCKET} --project=${PROJECT} --location=${REGION} \
  --uniform-bucket-level-access --public-access-prevention
mkdir -p /tmp/${ASR}-build
cat > /tmp/${ASR}-build/cloudbuild.yaml <<YAML
steps:
  - name: python:3.12-slim
    entrypoint: bash
    args:
      - -c
      - >-
        pip install -q "huggingface_hub>=1.5,<2" &&
        hf download ${HF_MODEL} --revision ${HF_REVISION} --exclude out_llm.safetensors --local-dir /workspace/${ASR} &&
        rm -rf /workspace/${ASR}/.cache
  - name: gcr.io/google.com/cloudsdktool/cloud-sdk:slim
    args: ["gcloud", "storage", "cp", "-r", "/workspace/${ASR}", "gs://${BUCKET}/"]
options:
  machineType: E2_HIGHCPU_8
timeout: 1800s
YAML
gcloud builds submit --project=${PROJECT} --region=${REGION} --config=/tmp/${ASR}-build/cloudbuild.yaml --no-source
```

## 2. サービスアカウントを作る

```shell
gcloud iam service-accounts create moqt-${ASR} --project=${PROJECT}
gcloud storage buckets add-iam-policy-binding gs://${BUCKET} \
  --member=serviceAccount:${SA} --role=roles/storage.objectViewer
```

## 3. デプロイする

```shell
gcloud beta run deploy moqt-${ASR} \
  --project=${PROJECT} --region=${REGION} --image=${VLLM_IMAGE} \
  --service-account=${SA} --no-allow-unauthenticated \
  --gpu=1 --gpu-type=nvidia-l4 --no-gpu-zonal-redundancy \
  --cpu=8 --memory=32Gi --no-cpu-throttling \
  --concurrency=8 --min-instances=0 --max-instances=1 --port=8080 --timeout=600 \
  --set-env-vars=HF_HUB_OFFLINE=1 \
  --add-volume=name=weights,type=cloud-storage,bucket=${BUCKET},readonly=true,mount-options=enable-buffered-read=true \
  --add-volume-mount=volume=weights,mount-path=/mnt/gcs \
  --startup-probe=httpGet.path=/health,httpGet.port=8080,initialDelaySeconds=5,periodSeconds=10,timeoutSeconds=5,failureThreshold=36 \
  --command=/bin/bash \
  --args="-c","timeout -k 10 240 cp -r /mnt/gcs/${ASR} /dev/shm/${ASR} || exit 1; exec vllm serve /dev/shm/${ASR} --served-model-name ${ASR} ${VLLM_ARGS} --host 0.0.0.0 --port 8080 --gpu-memory-utilization 0.85"
```

## 4. 書き起こし bot から呼ぶ

bot のサービスアカウント（手元ならそのアカウント）に `roles/run.invoker` を付け、bot に `--asr` で URL を渡します。
`--asr` は、デプロイしたモデルの数だけ繰り返せます。

```shell
gcloud run services add-iam-policy-binding moqt-${ASR} --project=${PROJECT} --region=${REGION} \
  --member=serviceAccount:<bot のサービスアカウント> --role=roles/run.invoker
ASR_URL=$(gcloud run services describe moqt-${ASR} --project=${PROJECT} --region=${REGION} --format 'value(status.url)')
uv run python -m moq_meeting_transcriber.bot --asr ${ASR}=${ASR_URL} --metadata-auth
```

手元の bot から呼ぶときは `--metadata-auth` の代わりに `--gcloud-auth` を付けます。

## 費用と起動時間

- インスタンスが起きている間だけ、サービスごとに課金されます。L4 1 台（8 vCPU・32 GiB）で約 $1.70/h です。
- 0 台からの最初のリクエストは、重みのコピー（Qwen3-ASR は約 4 GB、Granite Speech 2b は約 5 GB）と vLLM の起動を待ちます。その間、bot は `status` トラックで `starting` を出し、ページのバッジに表示されます。
