# MoQT Video Call Application

MoQT (Media over QUIC Transport) を使用したビデオ通話アプリケーションです。

## 概要

このアプリケーションは、draft-ietf-moq-transport-10 に準拠した MoQT プロトコルを使用して、リアルタイムのビデオ通話機能を提供します。

### 主な機能

- **Room & User 管理**: Room Name と User Name を入力してルームに参加
- **参加者グリッド表示**: ルーム内の他の参加者を一覧表示
- **選択的購読**: 参加者を選択して映像・音声を購読
- **メディア配信**: カメラ、マイク、画面共有を選択して配信
- **エンコード設定**: H.264 High@5.0 (avc1.640032) を含むコーデックを選択可能

## MoQT プロトコルフロー

### TrackName Space 構造

- **TrackName Space**: `/{RoomName}/{UserName}`
- **Video TrackName**: `video`
- **Audio TrackName**: `audio`

### メッセージシーケンス

1. **接続とセットアップ**
   - `SETUP` メッセージでサーバーと接続を確立

2. **ルーム参加**
   - `SUBSCRIBE_ANNOUNCES` で `/{RoomName}/` のANNOUNCEを購読
   - 既存参加者と新規参加者のANNOUNCEを受信

3. **メディア配信**
   - カメラ/マイクを選択
   - `ANNOUNCE` メッセージで `/{RoomName}/{UserName}` を通知
   - 他の参加者から `SUBSCRIBE` メッセージを受信
   - `SUBSCRIBE_OK` を返して配信開始
   - `OBJECT` メッセージでメディアデータを送信

4. **メディア受信**
   - 参加者カードで「Subscribe」ボタンをクリック
   - `SUBSCRIBE` メッセージで `/{RoomName}/{UserName}/video` と `/{RoomName}/{UserName}/audio` を購読
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
