# Python SDK と Rust SDK の設計

## ステータス
実装済み。`moqt::client` の公開 API、Python モジュールの公開 API、配布の手順を変える
変更で、このファイルも同時に更新する。

Rust の SDK は `moqt::client` を re-export する crate `moq-wasm`（`sdk/rust`）で、
設計は `docs/architecture/moq-wasm/sdk.md` にある。Python の SDK（core の上の再接続、
media、pipecat の transport）は `docs/architecture/python/sdk.md` にある。

## 目標
ブラウザ SDK（`docs/architecture/wasm/browser-sdk.md`）と同じことを、Python と Rust
でも同じ形で書けるようにする。relay に接続し、track を購読し、track を配信し、
namespace を監視する。基準は JS の `MoqSession`（`sdk/js/js/src/core/session.ts`）
で、3言語のメソッドは名前と引数の意味をそろえる。

このファイルの範囲は core API（session、購読、FETCH、配信、namespace）である。

## 3言語の API の対応

| JS（`MoqSession`） | Python（`moq_wasm.Session`） | Rust（`moqt::client::Session`） |
| --- | --- | --- |
| `MoqSession.connect(url, { token, versions })` | `moq_wasm.connect(url, token=, refresh_token=, verify_certificate=, versions=)` | `Session::connect(url, &ClientConfig)` |
| `subscribe(ns, track, { filter, forward, priority })` | `subscribe(ns, name, filter=, forward=, priority=, delivery_timeout=, group_order=)` | `subscribe(ns, name, SubscribeOptions)` |
| `subscribeCurrentGroup(ns, track, options?)` | `subscribe_current_group(ns, name, …)` | `subscribe_current_group` |
| `subscribeGroupStart(ns, track, newGroupAfter, options?)`、`MoqSubscription.requestNewGroup(largestGroup)` | `subscribe_group_start(ns, name, new_group_after, …)`、`Subscription.request_new_group` | `subscribe_group_start`、`Subscription::request_new_group` |
| `fetch(ns, track, start, end)` → `MoqFetch` | `fetch(ns, name, start, end)` → `Fetch` | `fetch` → `Fetch` |
| `trackStatus(ns, track)` → `MoqLocation \| undefined` | `track_status(ns, name)` → `Location \| None` | `track_status` |
| `watchNamespaces(prefix)` → `MoqNamespaceWatch` | `watch_namespaces(prefix)` → `NamespaceWatch` | `watch_namespaces` → `NamespaceWatch` |
| `announce(ns)` → `Announcement.close()` | `announce(ns)` → `Announcement.close()` | `announce` → `Announcement::close` |
| `publishTrack(ns, track, { retentionMs, publish })` → `TrackWriter` | `publish_track(ns, name, retention_seconds=, publish=, datagram=, priority=)` → `TrackWriter` | `publish_track(ns, name, PublishTrackOptions)` → `TrackWriter` |
| `TrackWriter.write(payload, newGroup, locHeader)` / `writeGroup(payload)` | `write(payload, new_group=)` / `write_group(payload)` | `write(payload, new_group, ExtensionHeaders)` / `write_group` |
| `TrackWriter.nextSubscriber()` | `next_subscriber()` | `next_subscriber` |
| `MoqSubscription` の async iteration | `Subscription` の async iteration | `Subscription::next` |
| `MoqSubscription.groups()` → `MoqGroup` | `groups()` → `Group` | `next_group` → `Group` |
| `MoqSubscription.largest` | `largest` | `largest` |
| `MoqSubscription.done`（PUBLISH_DONE） | `done()` | `done` |
| `setForward(forward)` / `setPriority(priority)` / `endBeforeGroup(group)` | `set_forward` / `set_priority` / `end_before_group` | `set_forward` / `set_priority` / `end_before_group` |
| `MoqSubscription.close()`（UNSUBSCRIBE） | `close()` | `close` |
| `ReceivedObject` の `groupId`、`subgroupId`、`objectId`、`objectStatus`、`objectPayload`、`locHeader` | `Object` の `group_id`、`subgroup_id`、`object_id`、`status`、`payload`、`extension_headers`、`timestamp_micros` | `ReceivedObject` |
| `RequestError`（draft のエラーコード） | `RequestError` の `message_type`、`code`、`reason` | `RequestError` |
| `closed` → `CloseInfo` | `closed()`。失敗で終わったときは例外を送出する | `closed` → `TransportClose` |
| `close()` | `close()` | `close`（NO_ERROR で閉じ、`Endpoint::wait_idle` まで待つ） |
| `broadcast(ns)` → `MediaBroadcast` | `moq_wasm.media.create_broadcast` | `moq_wasm::media::create_broadcast` |

## `moqt::client`: Rust の入口

`moqt` の `client` module に、`MoqSession` と同じ形の `Session` を置く。イベントの
振り分け（`modules/client/routes.rs` の `Routes`）はここにあり、wasm と Python は
その上の薄い binding である。

```rust
use bytes::Bytes;
use moqt::client::{PublishTrackOptions, Session, SubscribeOptions};
use moqt::ClientConfig;

let session = Session::connect("moqt://relay.example.com:4433", &ClientConfig::default()).await?;

let chat = session
    .publish_track(&["room-1", "alice"], "chat", PublishTrackOptions::default())
    .await?;
chat.write_group(Bytes::from_static(b"hello")).await?;

let mut subscription = session
    .subscribe(&["room-1", "bob"], "chat", SubscribeOptions::default())
    .await?;
while let Some(object) = subscription.next().await? {
    println!("{} {:?}", object.group_id, object.payload);
}

let mut watch = session.watch_namespaces(&["room-1"]).await?;
while let Some(event) = watch.next().await {
    println!("{:?} {}", event.namespace, event.active);
}

session.close().await;
```

型と、JS との対応:

| `moqt::client` | JS | 中身 |
| --- | --- | --- |
| `Session` | `MoqSession` | `Arc<moqt::Session>`、`Origin`、`NamespaceWatchers`、PUBLISH_DONE の待ち手、イベントループの `JoinHandle` を持つ。`close` はイベントループを止め、`moqt::Session::close` を呼ぶ |
| `Subscription` | `MoqSubscription` | `next`、`next_group`（JS の `groups()`）、`largest`、`done`、`set_forward`、`set_priority`、`request_new_group`、`end_before_group`、`close` |
| `Group` | `MoqGroup` | 1本の subgroup stream |
| `Fetch` | `MoqFetch` | 1つの FETCH の応答 |
| `NamespaceWatch` | `MoqNamespaceWatch` | `next` と `close`（UNSUBSCRIBE_NAMESPACE） |
| `TrackFollower` | なし | `follow_track` の戻り値。publisher が入れ替わる1本のトラック（ブラウザの視聴者が出す操作トラック）を追い、`next` で `FollowEvent`（オブジェクトか、追っていた publisher が抜けたこと）を返す。使うのはネイティブのブリッジだけなので、JS と Python には出さない |
| `Announcement` | `Announcement` | `close`（PUBLISH_NAMESPACE_DONE） |
| `TrackWriter` | `TrackWriter` | `Origin` に書く `write`（拡張ヘッダーは `ExtensionHeaders`）、`write_group`、`next_subscriber` |
| `EventHandler` | なし | 下記 |
| `RequestError` | `RequestError` | `kind`（`RequestKind`）、`failure`（`RequestFailure`）、`code()`（draft のエラーコード）、`reason()` |
| `PublishTrackError` | なし（`Error` の文字列） | `publish_track` の `anyhow::Error` に入る。`OtherDelivery`: 登録し直したトラックが、もう一方の `datagram` でオブジェクトを送っていた |

決めごと:

- 作り方は2つある。`Session::new(moqt::Session)` は、接続済みの session のイベントを
  引き取る。wasm と Python の binding は、endpoint（ブラウザの transport、トークン、
  telemetry の span）を自分で作ってから、これを呼ぶ。`Session::connect(url, &ClientConfig)`
  は、自分の endpoint で接続する（native は `NATIVE`、wasm32 は `BROWSER`）。

- 名前は JS に合わせる（`publish_track`、`watch_namespaces`、`announce`）。
  `moqt::TrackWriter` と `moqt::Subscription` は、1本の stream に書く低い層の型として
  残る。同じ名前になるが、`client::` で区別する。
- namespace は、JS と Python と同じく要素の列（`&[&str]`）で受け取る。`/` で
  つなぐのは module の中で行う。
- native では `Session` を `Send + Sync` にする。Python の binding が tokio の別の
  thread から使うためである。wasm32 でも同じ型を使う。中の表は `std::sync::Mutex`
  で持ち、lock を `.await` の間は握らない。task は `executor::spawn` で起こす。
- イベントループが知らない SUBSCRIBE には TRACK_DOES_NOT_EXIST を、PUBLISH には
  UNINTERESTED を返す。
- 再接続と GOAWAY は `moqt::Session` が行う。`client` は何も足さない。

### `broadcast` への振り分け

`broadcast` は `moqt` に依存するので、`moqt::client` は `broadcast` を知ることが
できない。そこで `async fn handle(&self, SessionEvent) -> Option<SessionEvent>`
（引き受けなければイベントを返す）を `client::EventHandler` trait にし、
`Session::add_handler` で `Origin` より前に並べる。`add_handler` が返す `HandlerId`
を `remove_handler` に渡すと外れる（Python の `MediaBroadcast.close` が使う）。
`broadcast::RelayConnection` はこの trait を実装し、
`RelayConnection::with_client_session` で `client::Session` から作る。

### Rust の入口を `moqt` の中に置く理由

| 案 | 良い点 | 悪い点 |
| --- | --- | --- |
| A. `moqt` をそのまま SDK にする | 何も足さない | 配信と購読を1つの session で行う利用者が、イベントループを自分で書く。wasm と Python が同じループを別々に持つ |
| B. 入口の crate を新しく作る | `moqt` の公開 API が増えない | 利用者は `moqt` とは別の名前の crate を足す。外向けの名前（`client` は一般的すぎる）を別に決める必要がある |
| **C. `moqt::client` module（採用）** | 利用者が足すのは `moqt` だけで、名前が3言語で揃う。重複が1か所になる | `moqt` の公開 API が増え、draft-18 への移行でも守る面が広がる |

B の利点だった「`broadcast` への振り分けを持てる」は、`EventHandler` で C でも
満たせる。Python の binding が wasm と同じ Rust の型を包むので、3言語の振る舞いの
違いは binding の層の型変換だけになる。

## Python の API

`sdk/python/python/moq_wasm/_native.pyi`（PyO3 の binding）と `_session.py`（その上の
`Session`）が定義する。名前は JS の `MoqSession` に合わせ、async のメソッドは asyncio
の awaitable を返す。設計は `docs/architecture/python/sdk.md` の「Core」にある。

## 配布

3言語とも、レジストリには公開せず、tag を打って GitHub Release で配る。JS と同じく、
1.0 までは API を壊してよく、試してくれる利用者と直接やり取りしながら進めるためで
ある。

### tag の名前
言語ごとに版を刻むので、tag には言語を付ける。

| 言語 | tag | Release に付けるもの |
| --- | --- | --- |
| JS | `js-sdk-v<version>` | `npm pack` の tarball |
| Python | `python-sdk-v<version>` | wheel |
| Rust | `rust-sdk-v<version>` | なし（tag そのものが依存先） |

### Rust
`docs/architecture/moq-wasm/sdk.md` の「Release」に書く。利用者は crate `moq-wasm`
（`sdk/rust`）を、tag を指す git 依存で入れる。

### Python
`docs/architecture/python/sdk.md` の「Package」に書く。wheel は Linux x86_64、
Linux aarch64、macOS arm64 の abi3 wheel（Python 3.11 以上）で、
`python-sdk-v<version>` の tag の GitHub Release に添付する。

### レジストリ（crates.io、PyPI、npm）に公開しない理由

- **API がまだ固まっていない。** 3言語とも 1.0 未満で、minor の版でも API を壊す。
  レジストリの利点（検索、範囲指定での更新、Dependabot）は、壊れない約束があって
  初めて生きる。
- **公開した版は消せない。** crates.io は yank しかできず、PyPI は同じ版のファイル
  を出し直せない。名前を一度公開すると、事実上固定される。
- **crates.io の `moqt` は使えない。** 2026-10-30 の時点で、別のプロジェクトが
  `moqt` 0.0.1 を公開している。crates.io に出すなら、crate の名前から決め直す必要が
  ある。PyPI と npm の `moqt` は、同じ時点で空いている。
- **Rust は依存の crate もすべて公開が要る。** crates.io の crate は git や path の
  依存を持てない。`moqt` 自身には path 依存は無いが、`broadcast` を出すなら `msf` と
  `mediapack` も出す必要がある。

公開は、core API が 1.0 になったときに3言語まとめて改めて決める。

## 名前と版

- **SDK の名前はリポジトリ名の `moq-wasm`。** npm のパッケージは今のまま
  `moq-wasm`、Python の配布名も `moq-wasm`（import は `moq_wasm`）にする。Python は
  WebAssembly を使わないが、名前は技術ではなくリポジトリに揃える。Rust の SDK の
  crate も `moq-wasm`（`sdk/rust`）である。
- **名前はレジストリに公開するまで変えられる。** 今の利用者は、tarball の URL と
  import を書き換えれば移れる。リポジトリ名（`moq-wasm`）を変えるかどうかも、公開を
  決めるときに一緒に決める。GitHub は、改名後も旧 URL を転送する。
- **版は言語ごとに独立させる。** 3つの SDK は、ライブラリとしてではなく relay との
  通信でしか関わらないので、版を揃える必要が無い。各 SDK は、自分が変わったときに
  版を上げる。session がネゴシエートする draft のバージョンは、各 changelog に書く。
- semver に従い、1.0 までは minor の版で壊してよい（ブラウザ SDK と同じ）。

## 決定事項

1. **Python の wheel: Linux x86_64、Linux aarch64、macOS arm64。** ほかの
   プラットフォームは、git source からのソースビルドで使える（動作の保証はしない）。
2. **Rust の入口: `moqt` の中の `client` module。** SDK の crate `moq-wasm` は
   これを re-export する（`docs/architecture/moq-wasm/sdk.md`）。
3. **名前: 3言語とも SDK は `moq-wasm`。**

## 範囲外

- **レジストリへの公開。** 理由と時期は「配布」と「名前と版」に書いた。
- **メディア API。** JS の `moq-wasm/media` に当たるもの。Python は `python/sdk.md`、
  Rust は `moq-wasm/sdk.md` で設計する。
- **Windows の wheel。** 必要になったら matrix に足す。Windows は
  `moqt` を動かしたことが無いので、足す前に CI で pytest を回せるようにする。
