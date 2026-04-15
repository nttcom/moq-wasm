# 1. タスクの目的と背景

## 目的
`moqt-server-sample` と `moqt-client-wasm` をローカルで**ワンコマンドでセットアップ**できるようにし、さらに `MoQT Media Examples` の **Publisher → Subscriber 間で映像が実際に届くこと**を **Playwright E2E テストで自動検証**できるようにする。

## 背景
現状は以下が手作業です。

- サーバー証明書の生成（`README.md`）
- `moqt-server-sample` の起動
- `js` 配下の `npm install`
- `wasm-pack` による `moqt-client-wasm` ビルド（`js/package.json` の `npm run wasm`）
- Vite サーバー起動
- Chrome を特殊オプション付きで起動して WebTransport / fake media を有効化する必要があるが、今回の自動化対象は **Linux 環境前提** として整理する必要がある（既存補助スクリプトは `scripts/chrome_mac.sh`）

また、E2E 自動化に向けて次の課題があります。

- Playwright 関連ファイル・依存が存在しない
- Publisher / Subscriber の接続先 URL 初期値が **リモート URL** (`https://moqt.research.skyway.io:4433`)
- Publisher / Subscriber の namespace 初期値が **現在時刻ベースで別々に生成**されるため、自動テストでは明示的に揃えないと疎通しない
- Publisher 画面に `getSampleVideo` ボタンがあるが、**現時点では実装されていない**
- 成功判定が主に `console` / 一部文言に依存しており、E2E 観点では安定した検証フックが弱い

---

# 2. 実装すべき具体的な機能や変更内容

## セットアップ用 Node.js スクリプト
リポジトリルートから実行できる Node.js スクリプトを追加し、最低限以下を自動化する。

1. `moqt-server-sample/keys` 配下の自己署名証明書生成（未作成時）
2. `js` 配下の依存インストール
3. `moqt-client-wasm` の `wasm-pack` ビルド実行（成果物は `js/pkg`）
4. Playwright の依存導入と Chromium 準備
5. 必要ツール不足時の明示的なエラー出力（例: `wasm-pack`, `cargo`, `npm`）

## E2E 実行用 Node.js スクリプト
別の Node.js スクリプトで、以下を**一括実行**する。

1. `moqt-server-sample` 起動
2. `js` の Vite サーバー起動
3. 起動完了待ち
4. Playwright E2E 実行
5. 終了時の子プロセス後始末

## Playwright E2E テスト
`MoQT Media Examples` の既存画面を使って、以下のシナリオを自動化する。

1. Publisher ページを開く
2. Subscriber ページを開く
3. 両者の URL を `https://127.0.0.1:4433` に設定
4. 両者の track namespace を同じ値に設定
5. Publisher: Connect → Setup → Announce
6. Subscriber: Connect → Setup → Catalog Subscribe → Track Subscribe
7. Publisher: fake media で `getUserMedia` → Send
8. Subscriber: Catalog 読み込み完了、track subscribe 完了、video object 受信、`video` 要素が再生状態になることを確認

## UI 側のテスト容易化
既存 UI に対して、必要最小限の E2E 向け改善を入れる。

- 安定したセレクタ追加（`data-testid` など）
- 接続状態 / announce 状態 / catalog 読み込み状態 / 受信状態の DOM 表示
- 必要であれば `getSampleVideo` を実装するか、使わないなら E2E は `getUserMedia + fake-device` に統一
- テスト中に remote URL が使われないよう、初期値上書き手段を持たせる

---

# 3. 技術的な実装アプローチ

## 推奨ファイル構成
- `scripts/setup-media-e2e.mjs`
- `scripts/run-media-e2e.mjs`
- `js/playwright.config.ts`
- `js/tests/media-e2e.spec.ts`

※ Node.js スクリプト本体はリポジトリルートの `scripts/`、Playwright 関連は既存 `js/package.json` に寄せるのが自然です。

## セットアップスクリプトの実装方針
`child_process.spawn` / `execFile` で既存コマンドを順に呼ぶ。

- 証明書生成: `openssl req ...` を Node.js から実行
- JS 依存: `npm install` (`cwd=js`)
- WASM ビルド: `npm run wasm` (`cwd=js`)
- Playwright browser install: `npx playwright install chromium`

## E2E 実行スクリプトの実装方針
Node.js から以下を並列起動して監視する。

- サーバー: `cargo run -p moqt-server-sample`
- フロント: `npm run dev -- --host 127.0.0.1 --port <固定ポート>`

待ち合わせ条件は以下が妥当です。

- サーバー: 標準出力に `Server ready!`
- フロント: 対象 URL に HTTP 200 が返る

## Playwright のブラウザ起動方針
`playwright.chromium` を使い、既存の `scripts/chrome_mac.sh` で有効化している内容を **Linux 上の Chromium 起動引数として** Playwright に移植する。

ここでいう「ワンコマンド実行」は **Linux 開発環境での再現性を担保すること** を主目的とし、macOS 向け補助スクリプトの互換実装まではスコープに含めない。

最低限必要な引数:

- `--origin-to-force-quic-on=127.0.0.1:4433`
- `--ignore-certificate-errors-spki-list=<cert.pem から算出した SPKI>`
- `--use-fake-device-for-media-stream`
- `--use-fake-ui-for-media-stream`
- `--autoplay-policy=no-user-gesture-required`

特に **WebTransport の自己署名証明書回避は `ignoreHTTPSErrors` だけでは不十分** なので、`cert.pem` から SPKI を計算して渡す実装が必要です。

## テストの成功判定
判定は画面文言と DOM 状態を優先し、必要に応じてログも補助的に使う。

推奨アサーション:

- Subscriber の `catalog-track-status` が `Catalog loaded:` を含む
- 同要素が `Subscribed video=` を含む
- Subscriber 側 `video` 要素の `readyState >= 2`
- Subscriber 側 `video.currentTime > 0`
- 必要なら `recv video object` 相当の状態表示が更新される

---

# 4. 考慮すべき制約や注意点

## ブラウザ制約
このフローは `WebTransport`, `WebCodecs`, `MediaStreamTrackProcessor`, `MediaStreamTrackGenerator` 前提なので、**Chromium 系ブラウザ固定**で考えるべきです。

加えて、E2E で必要な特殊起動オプション付きブラウザ起動は **Linux 前提** として扱い、少なくとも CI / 想定開発環境では Linux 上の Chromium / Chrome で成立することを完了条件に含めます。macOS 固有の起動補助は参考情報に留めます。

## 証明書制約
`moqt-server-sample` は `moqt-server-sample/keys/key.pem`, `cert.pem` が必須です。  
未作成時に自動生成し、Playwright 側にも同じ証明書由来の SPKI を渡す必要があります。

## URL 初期値の罠
Publisher / Subscriber ともに接続先初期値がリモート URL なので、E2E では必ず `https://127.0.0.1:4433` に上書きする必要があります。

## namespace 初期値の罠
Publisher / Subscriber の namespace 初期値は画面ロード時刻ベースです。  
2ページでずれる可能性があるため、テストでは**同一の固定 namespace を明示入力**する必要があります。

## 現状 UI の不足
- `getSampleVideo` ボタンは存在するが未実装
- Publisher は Subscriber ほど明示的な状態表示がない
- 成功/失敗が console 依存気味

そのため、E2E 安定化のために軽微な UI 補強が必要になる可能性が高いです。

## 実行環境依存
- `wasm-pack` が未導入だとセットアップ不能
- `4433` や Vite 用ポートが他プロセスと競合する可能性
- Headless 実行時の WebCodecs / media 周りの挙動差異が出る可能性
- Chrome / Chromium の特殊起動オプションは **Linux 前提** で設計し、OS ごとの差異吸収は今回の主目的に含めない

## 既存コード上の注意
Publisher は `Connect` 後に一部ボタンハンドラを登録しているため、再接続を繰り返すとイベント重複の温床になりえます。E2E は基本的に**各ページで1回だけ Connect**する前提で組むか、必要ならハンドラ登録位置を整理します。

---

# 5. 完了条件

以下を満たしたら完了です。

1. リポジトリルートから **1コマンド**でセットアップできる  
   - 証明書生成
   - `js` 依存導入
   - `moqt-client-wasm` のビルド
   - Playwright/Chromium 準備

2. リポジトリルートから **1コマンド**で E2E 実行できる  
   - `moqt-server-sample` 起動
   - Vite 起動
   - Playwright 実行
   - 後始末まで自動

3. E2E テストが **Publisher → Subscriber の実際の映像疎通**を確認している  
   - 単なるページ表示確認ではなく、
   - Catalog 読み込み
   - Track Subscribe
   - video object 受信
   - Subscriber の `video` 再生進行  
   まで確認できる

4. テストがローカル環境に依存しすぎず、少なくともこのリポジトリの想定開発環境で再現可能である  
   - 特にブラウザ起動オプション付き E2E は **Linux 環境で再現可能** であることを前提にする

5. 関連ドキュメント（最低でも `README.md` か `js` 側 README 相当）に実行方法が追記されている

6. 変更後の検証コマンドが整理されている  
   - Rust: `cargo fmt` / `cargo clippy` / `cargo test`
   - JS: `npx prettier --check "js/**/*.{js,jsx,ts,tsx,json,css,md}"`
   - E2E: Playwright 実行コマンド