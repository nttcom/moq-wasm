# Codex Configuration

## 要求

- 日本語で思考・回答してください
- 実装を開始する前に、どんなものを実装しようとしているか、整理して説明してください。
- 実装後は、JS では prettier, npm run lint, Rust では clippy fmt コマンドを実行して、linter を動かすようにしてください。また、テストも実行してください
- moqt-core は server/client の両方で参照するため、ここで定義された情報は client/server でも活用するようにしてください
- 実装を行う際は、処理の流れがわかりやすいように適宜コンポーネント化やクラス化、関数抽出を行い、合計の Line of Code が短くなるようにしてください。
  - 可能であれば、 1 ファイルは 100 行程度に収まる方が望ましいです。しかし、可読性や冗長性の観点から、この行数制限は無視しても構いません
- moqt-server, js のディレクトリの実装であっても、moqt-core や moqt-client-wasm の変更は可能なので、moqt-core や moqt-client-wasm の API の設計を変更した方が良い場合はそれを提案してください
- 修正を行った際は、各種ディレクトリのドキュメントにも修正を加えてください
  - AGENTS.md や.claude/CLAUDE.md にも修正を加えてください
- 作業が完了したタイミングで、以下のコマンドを実行して音を鳴らしてください
  - `bash -lc "echo 'Codex Needs Your Attention' | terminal-notifier -sound default"`

## プロトコル仕様

- specsフォルダに格納されているMedia over QUIC 関連のInternet Draftの仕様を適宜参照して実装してください

## プロジェクト概要

draft-ietf-moq-transport-10 に準拠した MoQ (Media over QUIC) の Rust 実装です。以下のモジュールで構成されています：

- **moqt-core**: サーバーとクライアントで共有されるコアライブラリ。メッセージハンドラ、データ構造、プロトコル実装を含む
- **moqt-server**: WebTransport (`wtransport` クレート使用) を用いたサーバーアプリケーション。PubSub ロールとオブジェクトキャッシュをサポート
- **moqt-server-sample**: PubSub 機能を示すサンプルサーバー実装
- **moqt-ingest-gateway**: 映像はキーフレームごとにメタデータへ codec を付与し、音声はすべての Object に codec/AudioSpecificConfig を載せるようにしてください
- **moqt-client-wasm**: WASM にコンパイルされるブラウザクライアント。WebTransport API を使用し Publisher/Subscriber/PubSub ロールをサポート
  - WASM にコンパイルされたものは js/pkg に出力され、js ディレクトリから参照される。
  - js ディレクトリに変更を加える際も、moqt-client-wasm は適宜参照し、改修を加えても良い
- **moqt-client-onvif**: RTSP/ONVIF で IP カメラに接続し、映像/音声の取り込みや制御信号送信を行うクライアント
- **js/**: GitHub Pages にデプロイされるブラウザデモ用のフロントエンド例とツール (Vite)

datagram と subgroup stream の配信モードをサポートし、setup、announce、subscribe、subscribe_announces などの制御メッセージに対応。サーバーは `make server`、クライアントは `make client` で起動します。

### js ディレクトリ

このディレクトリには、moqt-core およびそれに依存した moqt-client を参照して、様々な MoQT アプリケーションが実装されています。

- js/examples/message
  - MoQT メッセージの Client Server の挙動を正確にテストするためのアプリケーション
- js/examples/media
  - MoQT メッセージと映像・音声の Encoder/Decoder の挙動を正確にテストするためのアプリケーション
- js/examples/call
  - MoQT メッセージを利用した通話アプリケーション
- js/pkg
  - moqt-client-wasm ディレクトリを WASM コンパイルしたディレクトリ
  - js/pkg を参照する際は moqt-client-wasm ディレクトリも参照すること

このディレクトリの開発を行う場合には、js/pkg のライブラリは moqt-core, moqt-client-wasm ディレクトリによって出力されるものであるため、moqt-core, moqt-client-wasm も参照し、それも含めた設計と実装を行なってください。場合によっては moqt-core, moqt-client-wasm の修正も検討してください。

## 更新メモ

- マージコンフリクト解消時は moqt-client-wasm の依存関係と JS メディア購読 UI の整合を優先する。
