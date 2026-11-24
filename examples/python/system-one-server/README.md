# system-one-server

決定モデル [d1-2B](https://huggingface.co/LiquidAI/d1-2B) で `/v1/systemone` を提供する HTTP サーバです（Chat Moderation / Camera Detection / PTZ Tracking で共用）。

| パス | 内容 |
| --- | --- |
| `GET /health` | モデル読み込み完了後に `200` |
| `POST /v1/systemone` | `{"state", "images": [data URL, ...], "questions": {name: question}}` に対し `{"answers", "usage"}` を返却（不正質問は `422`） |

- 質問形式（`noul`, `choice`, `boxes` 等）は d1-2B モデルカードを参照。
- 推論は 1 件ずつ直列実行。デバイスは CUDA → MPS → CPU の順で自動選択（CPU のみ `float32`）。

## ローカル起動

初回は Hugging Face から重み（約 3.8 GB）を取得します。

```shell
cd examples/python/system-one-server
uv run system-one-server --port 8000
```

bot に `--system-one-url http://127.0.0.1:8000/v1/systemone` を渡します（Apple M1 Pro MPS で `boxes` 1 問・1 枚あたり約 0.9 秒）。
Cloud Run デプロイは [system-one-on-cloud-run.md](../system-one-on-cloud-run.md) を参照してください。

## テスト

```shell
uv run pytest
```
