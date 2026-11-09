# system-one-server

MoQ Chat Moderation・MoQ Camera Detection・MoQ PTZ Tracking の bot が呼ぶ System One の `/v1/systemone` を、決定モデル [d1-2B](https://huggingface.co/LiquidAI/d1-2B) で答える HTTP サーバです。

| パス | 内容 |
| --- | --- |
| `GET /health` | モデルを読み込み終えたら 200 |
| `POST /v1/systemone` | `{"state", "images": [data URL, ...], "questions": {名前: 質問}}` に、モデルの `{"answers", "usage"}` で答える。モデルが受け付けない質問は 422 |

- 質問の形式（`noul`・`choice`・`boxes` など）は d1-2B のモデルカードにあります。
- 推論は 1 件ずつ行います。
- CUDA、MPS、CPU の順に使えるものを選び、CPU のときだけ float32 で読み込みます。

## 手元で起動する

初回は重み（約 3.8 GB）を Hugging Face からダウンロードします。

```shell
cd examples/python/system-one-server
uv run system-one-server --port 8000
```

bot には `--system-one-url http://127.0.0.1:8000/v1/systemone` を渡します。Apple M1 Pro（MPS）では、`boxes` 1 問の画像 1 枚に約 0.9 秒かかります。

Cloud Run へのデプロイは [d1-2B を Cloud Run にデプロイする](../system-one-on-cloud-run.md) にあります。

## テスト

```shell
uv run pytest
```
