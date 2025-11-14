# RTMPS ingest 環境の立ち上げ手順

`infra/nginx-rtmp` ディレクトリには stream モジュール + nginx-rtmp モジュールを有効化した nginx を Docker から起動するためのセットアップ一式が入っています。自己署名証明書を使って RTMPS で `ffmpeg` から配信し、nginx が受信できることを確認するまでの手順をまとめました。

## 1. 前提

- Docker / Docker Compose v2 が利用できること
- `openssl` / `ffmpeg` コマンドがローカルにインストール済みであること

## 2. 証明書の発行

```bash
# リポジトリルートで実行
./scripts/generate_rtmps_cert.sh
```

`infra/nginx-rtmp/certs/server.crt` および `server.key` が生成されます。`OPENSSL_SUBJECT` や `CERT_DAYS` を環境変数で与えると CN や有効期限を変更できます。

## 3. nginx-rtmp の起動

```bash
cd infra/nginx-rtmp
docker compose build
docker compose up -d
```

- `docker compose build` で `alpine` ベースの軽量イメージに `nginx` + `nginx-mod-rtmp` をインストールし、`stream` モジュールも利用可能にしています。
- `./nginx.conf` がそのままコンテナ内の `/etc/nginx/nginx.conf` にマウントされます。
- `stream` モジュールで 1935/TCP を RTMPS 終端し、プレーン RTMP (1936/TCP) の nginx-rtmp サーバーへプロキシしています。8080/TCP は疎通確認用 HTTP エンドポイント (および `rtmp_stat`) です。
- 受信したストリームは `/var/media/live` に FLV として保存され、ホスト側では `infra/nginx-rtmp/recordings` に展開されます。
- 録画ディレクトリは事前に `mkdir -p infra/nginx-rtmp/recordings/live` で作成しておくと確実です（コンテナからの書き込みエラーを避けられます）。

ログを確認したい場合は `docker compose logs -f rtmp` を実行してください。`infra/nginx-rtmp/logs` にもコンテナ内の `/var/log/nginx` が同期されます。

## 4. ffmpeg から RTMPS 配信

ローカルに動画ファイルがなくても、`testsrc` と `sine` フィルターで疑似映像/音声を生成できます:

```bash
ffmpeg -re \
  -f lavfi -i "testsrc=size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=1000:sample_rate=48000" \
  -c:v libx264 -preset veryfast -tune zerolatency \
  -c:a aac -b:a 128k \
  -pix_fmt yuv420p \
  -f flv "rtmps://localhost:1935/live/test"
```

- `live` は nginx 側で定義しているアプリケーション名です。
- パスの末尾 (`test`) がストリームキーとして扱われます。
- 送信開始後、`docker compose logs -f rtmp` に `publish` / `record` に関するログが出力されます。また `infra/nginx-rtmp/recordings/live` 配下に録画ファイルが生成されます。

## 5. 受信確認

1. HTTP 疎通確認: `curl http://localhost:8080/` または `http://localhost:8080/stat` でステータス XML を取得できます。
2. 録画ファイル: `ls infra/nginx-rtmp/recordings/live` でファイルが作成されているか確認します。
3. nginx ログ: `infra/nginx-rtmp/logs/access.log` / `error.log` を参照します。

これで「RTMPS で配信 → nginx-rtmp が受信し FLV を記録する」状態をローカルで再現できます。今後はこの ingest パイプラインの先に MoQ Gateway を接続していく想定です。
