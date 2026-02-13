# MoQT Video Call Application

MoQT (Media over QUIC Transport) を使用したビデオ通話アプリケーションです。

## 概要

このアプリケーションは、draft-ietf-moq-transport-10 に準拠した MoQT プロトコルを使用して、リアルタイムのビデオ通話機能を提供します。

### 主な機能

- **Room & User 管理**: Room Name と User Name を入力してルームに参加
- **Relay 選択**: 127.0.0.1 かドメインの MoQT relay を選択
- **Catalog 経由購読**: `catalog` track から video/audio track を解決して購読
- **動的 Catalog 構成**: 初期Catalogは空。camera/mic/screenshare を ON にしたとき未登録なら初期track群を追加し、OFFでは自動削除しない
- **Catalog プロファイル**: camera は 1080p/720p/480p、audio は 128/64/32kbps、screenshare は 1080p/720p/480p
- **参加者グリッド表示**: ルーム内の他の参加者を一覧表示
- **選択的購読**: `Catalog Subscribe` 後に video/audio を個別選択し、`Subscribe Video` / `Subscribe Audio` で個別購読
- **メディア配信**: カメラ、マイク、画面共有を選択して配信
- **エンコード設定**: H.264 High@5.0 (avc1.640032) を含むコーデックを選択可能
- **設定モーダル分離**: `Device` と `Catalog` のボタンを分け、`Select Devices` モーダル（device/getUserMedia）と `Catalog Details` モーダル（track詳細）を独立表示
- **Catalog Track 編集**: `Catalog Details` から Track 情報を複数追加・削除・編集（`Video / Screenshare / Audio` のグループ表示、codec/bitrate/resolution/channel はプリセット選択）
- **Catalog詳細表示**: `Catalogs` モーダルの Track 一覧で codec/bitrate/resolution/samplerate/channel/live を表示
- **デバッグログ最適化**: subscriber/decoder のログを初回受信・設定変更・警告中心に絞り、object単位の大量ログを抑制
- **AV1初期化修正**: video decoder 初期化時に Catalog の codec を優先し、AV1 track 購読時に `avc1` へ誤フォールバックしない
- **Decoder初期化方針**: Catalog情報を初回decoder初期化に固定利用し、decode中の動的reinitializeと既定codecフォールバックを行わない
- **独立エンコード解決**: Catalog からのエンコード設定解決は `camera` と `screenshare` を別々に扱い、片方のtrack設定がもう片方へ混入しない
- **購読時codec適用**: Subscriber は選択した Catalog track の codec を decoder 初期化に反映し、screenshare 単独購読時も正しい codec でデコードする
- **購読時profile反映**: Publisher は incoming SUBSCRIBE の track 名に対応する Catalog profile を source の encoder 設定へ反映し、選択 bitrate/profile で送信する
- **Track単位Encoder**: Publisher は camera/screen/audio の各 Catalog track ごとに encoder worker を分離し、track 単位の設定（codec/bitrate 等）で送信する
- **保守性改善**: Subscriber の Catalog 購読UIは role 定義ベース（video/screenshare/audio）で共通描画し、Hook 側の Catalog add/remove も source 別の共通更新処理に統一

## MoQT プロトコルフロー

### TrackName Space 構造

- **TrackName Space**: `/{RoomName}/{UserName}`
- **Catalog TrackName**: `catalog`
- **Camera TrackName**: `camera_1080p`, `camera_720p`, `camera_480p`
- **Screenshare TrackName**: `screenshare_1080p`, `screenshare_720p`, `screenshare_480p`
- **Audio TrackName**: `audio_128kbps`, `audio_64kbps`, `audio_32kbps`

### メッセージシーケンス

1. **接続とセットアップ**

   - `SETUP` メッセージでサーバーと接続を確立

2. **ルーム参加**

   - `SUBSCRIBE_ANNOUNCES` で `/{RoomName}/` のANNOUNCEを購読（UIのANNOUNCEハンドラ登録後に送信）
   - 既存参加者と新規参加者のANNOUNCEを受信

3. **メディア配信**

   - 初期状態では Catalog track は空
   - カメラ ON で camera 3段、マイク ON で audio 3段、画面共有 ON で screenshare 3段を Catalog に追加

   - OFF にしたメディアの track は Catalog から自動削除しない（ミュート用途を想定）
   - ON中でも Catalog Tracks 画面から任意のtrackを手動削除でき、その設定を維持する
   - `ANNOUNCE` メッセージで `/{RoomName}/{UserName}` を通知
   - Catalog subscribe を受けたら `catalog` track へ Catalog object を返却
   - Catalog が更新されたら Catalog object を再送し、Track 追加・削除を通知
   - 他の参加者から `SUBSCRIBE` メッセージを受信
   - `SUBSCRIBE_OK` を返して配信開始
   - `OBJECT` メッセージでメディアデータを送信

4. **メディア受信**
   - ルーム参加時には自動で SUBSCRIBE しない
   - 参加者カードで `Catalog Subscribe` を実行し、`catalog` から track 一覧を取得
   - Catalog の追加/削除更新も継続受信し、UI に反映
   - video/audio の track を選択し、`Subscribe Video` / `Subscribe Audio` で個別購読を実行
   - `OBJECT` メッセージでメディアデータを受信

## セットアップ

### 依存関係のインストール

```bash
cd ../../
npm install
```

### 開発サーバーの起動

```bash
npm run dev
```

ブラウザで http://localhost:5173/examples/call/ を開いてください。

### ビルド

```bash
npm run build
```

## プロジェクト構造

```
src/
├── components/
│   ├── ui/               # shadcn/ui コンポーネント
│   │   ├── button.tsx
│   │   ├── card.tsx
│   │   ├── input.tsx
│   │   └── label.tsx
│   ├── JoinRoomForm.tsx      # ルーム参加フォーム
│   ├── CallRoom.tsx          # 通話ルームメイン画面
│   ├── ParticipantCard.tsx   # 参加者カード
│   └── PublishMediaPanel.tsx # メディア配信パネル
├── hooks/
│   └── useLocalSession.ts    # MoQT セッション管理フック
├── types/
│   └── moqt.ts               # 型定義
├── lib/
│   └── utils.ts              # ユーティリティ関数
├── App.tsx                   # メインアプリケーション
└── main.tsx                  # エントリーポイント
```

## TODO: WASM統合

現在、MoQT クライアントの実装は `src/hooks/useLocalSession.ts` でスタブ化されています。
実際の WASM モジュール (`moqt-client-wasm`) を統合するには、以下を実施してください：

1. WASM モジュールのビルド

   ```bash
   cd ../../../moqt-client-wasm
   wasm-pack build --target web --features web_sys_unstable_apis
   ```

2. `useLocalSession.ts` 内のコメントアウトされたコードを有効化

   - `import init, { MOQTClient }` のインポート
   - `init()` の呼び出し
   - `MOQTClient` インスタンスの作成

3. エンコーダー/デコーダーワーカーの実装
   - 既存の `media/publisher` と `media/subscriber` のコードを参考に実装

## 技術スタック

- **React 19**: UI フレームワーク
- **TypeScript**: 型安全な開発
- **Tailwind CSS**: スタイリング
- **shadcn/ui**: UIコンポーネントライブラリ
- **Vite**: ビルドツール
- **MoQT (WASM)**: Media over QUIC Transport プロトコル実装
