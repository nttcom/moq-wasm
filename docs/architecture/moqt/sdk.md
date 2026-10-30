# Python SDK と Rust SDK の設計

## ステータス
提案。「決定事項」の3点は決めた。実装は「移行計画」の手順で行う。採用後は、
`moqt::client` の公開 API、Python モジュールの公開 API、配布の手順を変える変更で、
このファイルも同時に更新する。

## 目標
ブラウザ SDK（`docs/architecture/wasm/browser-sdk.md`）と同じことを、Python と Rust
でも同じ形で書けるようにする。relay に接続し、track を購読し、track を配信し、
namespace を監視する。基準は JS の `MoqSession`（`crates/wasm/js/src/core/session.ts`）
で、3言語のメソッドは名前と引数の意味をそろえる。

範囲は core API（session、購読、FETCH、配信、namespace）だけにする。JS の
`moq-wasm/media`（`publish()`、`LivePlayer`）はブラウザの capture と描画に結びついて
いるので、Python と Rust には同じものを作らない。Rust でメディアを配信するときは、
今と同じく `crates/broadcast` を使う。

## 現状

| 言語 | 入口 | 状態 |
| --- | --- | --- |
| JS | `moq-wasm` の `MoqSession` | SDK として完成した。0.1.0 を tag `sdk-v0.1.0` の GitHub Release で配っている |
| Python | `crates/python`（PyO3 + maturin、モジュール `moqt`、abi3-py311） | Meeting の transcriber と AI bot が要るものだけがある。`fetch`、`track_status`、トークン、購読のオプションが無い。bot は path 依存で、利用者は Rust でビルドする |
| Rust | `crates/moqt` | 制御メッセージ単位の API（`Session`、`Subscriber`、`Publisher`、`publish::Origin`、`namespace::NamespaceWatchers`）。`README.md` は commit を指す git 依存を案内している |

`MoqSession` 相当の処理は、Rust の中で2か所に重複している。

- `crates/wasm/src/session_routes.rs` の `SessionRoutes`
- `crates/python/src/session.rs` の `route_events`

どちらも `moqt::Session::receive_event` を1本のループで読む。そして、配信している
track への SUBSCRIBE、FETCH、TRACK_STATUS を `Origin::handle` へ、PUBLISH_NAMESPACE を
`NamespaceWatchers::handle` へ渡し、残りの SUBSCRIBE と PUBLISH を拒否する。wasm だけは、
PUBLISH_DONE を該当する購読に届ける処理と、`broadcast::RelayConnection` への振り分けも
持つ。Rust の利用者は、配信と購読を1つの session で行うなら、同じループを自分で書く
ことになる。

## 3言語の API の対応

左が JS（基準）、中が今の Python と Rust で、どう書くか、足りないものを示す。
Rust の列は `moqt` の今の API で同じことをする方法である。

| JS（`MoqSession`） | Python（今） | Rust（今の `moqt`） |
| --- | --- | --- |
| `MoqSession.connect(url, { token })` | `moqt.connect(url, verify_certificate=)`。**トークンが無い** | `Endpoint::create_client(&ClientConfig { authorization_token, .. })` → `connect(url).await?.await?`。イベントループは自分で書く |
| `subscribe(ns, track, { filter, forward, priority })` | `subscribe(ns, name)`。**filter、forward、priority が無い** | `Subscriber::subscribe(ns, name, SubscribeOption)` → `accept_data_receiver` → `TrackReader::new` |
| `subscribeCurrentGroup(ns, track)` | `subscribe(ns, name, current_group=True)` | `Subscriber::subscribe_current_group` が `(Subscription, TrackReader)` を返す |
| `fetch(ns, track, start, end)` → `MoqFetch` | **無い** | `Subscriber::fetch` → `accept_fetch_receiver` |
| `trackStatus(ns, track)` → `MoqLocation \| undefined` | **無い** | `Subscriber::track_status` の `content_exists` |
| `watchNamespaces(prefix)` → `MoqNamespaceWatch` | `subscribe_namespace(prefix)` → `NamespaceEvents`。**止める手段が無い** | `NamespaceWatchers::watch` → `Subscriber::subscribe_namespace`。`unsubscribe_namespace` で止める。振り分けは自分で書く |
| `announce(ns)` → `Announcement.close()` | `publish_namespace(ns)`。**取り下げ（PUBLISH_NAMESPACE_DONE）が無い** | `Publisher::publish_namespace` / `publish_namespace_done` |
| `publishTrack(ns, track, { retentionMs, publish })` → `TrackWriter` | `track(ns, name, retention_seconds=)` → `TrackPublication`。**PUBLISH が無い** | `Origin::ensure_track`、`Origin::publish`。SUBSCRIBE と FETCH への応答は `Origin::handle` をループで呼ぶ |
| `TrackWriter.write(payload, newGroup, locHeader)` / `writeGroup(payload)` | `write(payload, new_group=)`。**`write_group` と拡張ヘッダーが無い** | `Origin::write(OutgoingObject)` / `Origin::write_group` |
| `TrackWriter.nextSubscriber()` | **無い** | ループの中で、`Origin::handle` が SUBSCRIBE を引き受けたかを見る |
| `MoqSubscription` の async iteration | `Track` の async iteration | `TrackReader::next_object` |
| `MoqSubscription.groups()` → `MoqGroup` | **無い** | `StreamDataReceiverFactory` |
| `MoqSubscription.largest` | **無い** | `Subscription::content_exists`（SUBSCRIBE_OK の Largest Location） |
| `MoqSubscription.done`（PUBLISH_DONE） | **無い**（iterator が終わるだけ） | `SessionEvent::PublishDone` をループで受ける |
| `setForward(forward)` / `endBeforeGroup(group, forward)` | **無い** | `Subscriber::update_forward` / `end_before_group` |
| `MoqSubscription.close()`（UNSUBSCRIBE） | `Track.unsubscribe()` | `Subscriber::unsubscribe` |
| `ReceivedObject` の `groupId`、`subgroupId`、`objectId`、`objectStatus`、`objectPayload`、`locHeader` | `Object` の `group_id`、`object_id`、`payload`。**subgroup、status、拡張ヘッダーが無い** | `TrackObject` |
| `RequestError`（draft のエラーコード） | **無い**（`RuntimeError` の文字列） | `RequestFailure::Rejected { code, .. }` |
| `closed` → `CloseInfo` | `closed()`。**code と reason が無い** | `Session::closed` → `TransportClose` |
| `close()` | `close()` | `close_with_error` → `Endpoint::wait_idle` |
| `broadcast(ns)` → `MediaBroadcast` | 範囲外 | `broadcast::RelayConnection` と `BroadcastWriter` |

## `moqt::client`: Rust の入口

`moqt` に `client` module を足し、`MoqSession` と同じ形の `Session` を置く。
`SessionRoutes` と `route_events` の振り分けはここに移し、wasm と Python はその上の
薄い binding にする。

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
| `Session` | `MoqSession` | `Endpoint`、`Arc<moqt::Session>`、`Origin`、`NamespaceWatchers`、PUBLISH_DONE の待ち手、イベントループの `JoinHandle` を持つ。`close` は `close_with_error` のあと `Endpoint::wait_idle` まで待つ |
| `Subscription` | `MoqSubscription` | `next`、`groups`、`largest`、`done`、`set_forward`、`end_before_group`、`close` |
| `Group` | `MoqGroup` | 1本の subgroup stream |
| `Fetch` | `MoqFetch` | 1つの FETCH の応答 |
| `NamespaceWatch` | `MoqNamespaceWatch` | `next` と `close`（UNSUBSCRIBE_NAMESPACE） |
| `Announcement` | `Announcement` | `close`（PUBLISH_NAMESPACE_DONE） |
| `TrackWriter` | `TrackWriter` | `Origin` に書く `write`、`write_group`、`next_subscriber` |
| `EventHandler` | なし | 下記 |

決めごと:

- 名前は JS に合わせる（`publish_track`、`watch_namespaces`、`announce`）。
  `moqt::TrackWriter` と `moqt::Subscription` は、1本の stream に書く低い層の型として
  残る。同じ名前になるが、`client::` で区別する。
- namespace は、JS と Python と同じく要素の列（`&[&str]`）で受け取る。`/` で
  つなぐのは module の中で行う。
- native では `Session` を `Send + Sync` にする。Python の binding が tokio の別の
  thread から使うためである。wasm32 でも同じ型を使う。中の表は `std::sync::Mutex`
  で持ち、lock を `.await` の間は握らない。task は `executor::spawn` で起こす。
- イベントループが知らない SUBSCRIBE には TRACK_DOES_NOT_EXIST を、PUBLISH には
  UNINTERESTED を返す（今の wasm と Python と同じ）。
- 再接続と GOAWAY は、今と同じく `moqt::Session` が行う。`client` は何も足さない。

### `broadcast` への振り分け

wasm の `SessionRoutes` は、イベントを `Origin` より先に、ページの各
`broadcast::RelayConnection` に渡している。`broadcast` は `moqt` に依存するので、
`moqt::client` は `broadcast` を知ることができない。ただ、`Origin::handle`、
`NamespaceWatchers::handle`、`RelayConnection::handle` は、どれも
`async fn handle(&self, SessionEvent) -> Option<SessionEvent>`（引き受けなければ
イベントを返す）という同じ形をしている。そこで、この形を `client::EventHandler`
trait にし、`Session::add_handler` で `Origin` より前に並べられるようにする。
`broadcast` は `RelayConnection` にこの trait を実装し、`client::Session` から
`RelayConnection` を作る関数を足す（今の `with_session(session, origin)` の
引数を、`client::Session` が渡す）。

### Rust の入口を `moqt` の中に置く理由

| 案 | 良い点 | 悪い点 |
| --- | --- | --- |
| A. `moqt` をそのまま SDK にする | 何も足さない | 配信と購読を1つの session で行う利用者が、イベントループを自分で書く。wasm と Python の重複が残る |
| B. 入口の crate を新しく作る | `moqt` の公開 API が増えない | 利用者は `moqt` とは別の名前の crate を足す。外向けの名前（`client` は一般的すぎる）を別に決める必要がある |
| **C. `moqt::client` module（採用）** | 利用者が足すのは `moqt` だけで、名前が3言語で揃う。重複が1か所になる | `moqt` の公開 API が増え、draft-18 への移行でも守る面が広がる |

B の利点だった「`broadcast` への振り分けを持てる」は、`EventHandler` で C でも
満たせる。Python の binding が wasm と同じ Rust の型を包むので、3言語の振る舞いの
違いは binding の層の型変換だけになる。

## Python SDK に足す API

`moqt.pyi` を、JS の `MoqSession` に合わせて次の形にする。async のメソッドは今と同じ
く asyncio の awaitable を返す。

```python
async def connect(
    url: str,
    *,
    token: str | Callable[[], Awaitable[str]] | None = None,
    verify_certificate: bool = True,
) -> Session: ...

class Session:
    async def subscribe(
        self, namespace: list[str], name: str, *,
        filter: Literal["largest_object", "next_group_start"] = "largest_object",
        forward: bool = True,
        priority: int = 128,
    ) -> Subscription: ...
    async def subscribe_current_group(self, namespace: list[str], name: str) -> Subscription: ...
    async def fetch(self, namespace: list[str], name: str, start: Location, end: Location) -> Fetch: ...
    async def track_status(self, namespace: list[str], name: str) -> Location | None: ...
    async def watch_namespaces(self, prefix: list[str]) -> NamespaceWatch: ...
    async def announce(self, namespace: list[str]) -> Announcement: ...
    async def publish_track(
        self, namespace: list[str], name: str, *,
        retention_seconds: float = 60.0,
        publish: bool = False,
    ) -> TrackWriter: ...
    async def closed(self) -> CloseInfo: ...
    async def close(self) -> None: ...

class Subscription(AsyncIterator[Object]):   # async with で抜けると UNSUBSCRIBE
    largest: Location | None
    async def done(self) -> PublishDone | None: ...
    def groups(self) -> AsyncIterator[Group]: ...
    async def set_forward(self, forward: bool) -> None: ...
    async def end_before_group(self, group_id: int, forward: bool) -> None: ...
    async def close(self) -> None: ...

class TrackWriter:
    async def write(self, payload: bytes, *, new_group: bool = False) -> int | None: ...
    async def write_group(self, payload: bytes) -> int | None: ...
    async def next_subscriber(self) -> bool: ...

class Object:
    group_id: int
    subgroup_id: int
    object_id: int
    status: int | None
    payload: bytes

class RequestError(Exception):
    message_type: str   # "SUBSCRIBE" など
    code: int           # draft-14 のエラーコード
    reason: str
```

今の API から変わるもの:

- **名前を JS に合わせる。** `subscribe_namespace` → `watch_namespaces`、
  `publish_namespace` → `announce`、`track` → `publish_track`、`Track` →
  `Subscription`、`TrackPublication` → `TrackWriter`、`unsubscribe` → `close`。
  `current_group=True` は `subscribe_current_group` にする。`current_group` は
  filter を Largest Object に決めるので、`filter=` と同じメソッドに並べると矛盾する
  組み合わせができる（JS が別のメソッドにしているのも同じ理由）。
- **足すもの。** トークン（固定値か async 関数。関数は moqt の
  `AuthorizationTokenSource::provider` から、接続のたびに Python の event loop で
  呼ぶ）、購読のオプション、`fetch`、`track_status`、`Subscription` の `done`・
  `groups`・`largest`・`set_forward`・`end_before_group`、`Announcement.close`、
  `publish_track(publish=True)`、`write_group`、`next_subscriber`、`RequestError`、
  `CloseInfo`、`Object` の `subgroup_id` と `status`。
- **見送るもの。** LOC の拡張ヘッダー（JS の `locHeader`）。読む bot も書く bot も
  まだ無い。足すときは JS と同じく LOC の property として出し、変換は `mediapack`
  の `loc` module に任せる。

`examples/python` の bot は、名前を変える PR で一緒に書き換える。

## 配布

3言語とも、レジストリには公開せず、tag を打って GitHub Release で配る。JS と同じく、
1.0 までは API を壊してよく、試してくれる利用者と直接やり取りしながら進めるためで
ある。

### tag の名前
言語ごとに版を刻むので、Python と Rust の tag には言語を付ける。JS は今の
`sdk-v*` を続ける。

| 言語 | tag | Release に付けるもの |
| --- | --- | --- |
| JS | `sdk-v<version>`（今のまま） | `npm pack` の tarball |
| Python | `python-v<version>` | wheel |
| Rust | `rust-v<version>` | なし（tag そのものが依存先） |

### Rust
利用者は tag を指す git 依存で入れる。今の README は commit（`rev`）を案内している
ので、tag に変える。

```toml
[dependencies]
moqt = { git = "https://github.com/nttcom/moq-wasm", tag = "rust-v0.1.0" }
```

リリースの手順:

1. `crates/moqt/Cargo.toml` の version を上げ、`crates/moqt/CHANGELOG.md` にその
   version の節を足す。
2. `rust-v<version>` の tag を push する。
3. CI（`.github/workflows/rust-release.yml`）が、tag と version の一致と changelog の
   節を確かめる。そして `cargo update` のあとで `cargo test -p moqt` を回し、changelog
   の該当部分を本文にした GitHub Release を作る。

`cargo update` を挟むのは、git 依存の利用者にはこのリポジトリの `Cargo.lock` が
効かないからである。利用者は、依存の semver の範囲で最新の版を引く。CI でも同じ
解決をしてテストする。`cargo update` は runner の中だけで行い、commit しない。

### Python
tag で CI が wheel をビルドし、GitHub Release に添付する。abi3-py311 なので、
Python 3.11 以降に1つの wheel が使え、ビルドするのは OS と CPU の組み合わせの数だけで
ある。

| プラットフォーム | runner | wheel |
| --- | --- | --- |
| Linux x86_64 | `ubuntu-latest`（manylinux_2_28 のコンテナ） | `moqt-<version>-cp311-abi3-manylinux_2_28_x86_64.whl` |
| macOS arm64 | `macos-latest`（Apple Silicon） | `moqt-<version>-cp311-abi3-macosx_11_0_arm64.whl` |

利用者は、`pyproject.toml` に wheel の URL を platform の marker 付きで書く。pip と
uv のどちらでも入り、Rust は要らない。

```toml
[project]
dependencies = [
  "moqt @ https://github.com/nttcom/moq-wasm/releases/download/python-v0.1.0/moqt-0.1.0-cp311-abi3-manylinux_2_28_x86_64.whl ; sys_platform == 'linux' and platform_machine == 'x86_64'",
  "moqt @ https://github.com/nttcom/moq-wasm/releases/download/python-v0.1.0/moqt-0.1.0-cp311-abi3-macosx_11_0_arm64.whl ; sys_platform == 'darwin' and platform_machine == 'arm64'",
]
```

wheel の無いプラットフォーム（Linux aarch64、Windows など）では、uv の git source で
同じ tag からソースでビルドできる。Rust の toolchain が要り、動作は確かめていない。

```toml
[tool.uv.sources]
moqt = { git = "https://github.com/nttcom/moq-wasm", tag = "python-v0.1.0", subdirectory = "crates/python" }
```

リリースの手順:

1. `crates/python/pyproject.toml` の version を上げ、`crates/python/CHANGELOG.md` に
   その version の節を足す。
2. `python-v<version>` の tag を push する。
3. CI（`.github/workflows/python-release.yml`）が tag と version の一致と changelog の
   節を確かめ、matrix で wheel をビルドする。各 runner で、ビルドした wheel を入れた
   環境で pytest を回す。Docker の要る GOAWAY のテストは Linux だけで回す。そのあと、
   すべての wheel を1つの GitHub Release に添付する。

wheel のビルドには maturin を使う（今の build backend と同じ）。CI の action は、
ほかの workflow と同じく commit SHA で固定する。

`examples/python` の bot は、リポジトリの中では今と同じく path 依存でビルドする。
Release の wheel を使うのは、リポジトリの外の利用者である。

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

- **名前は3言語とも `moqt`。** Rust の crate、Python のモジュールと配布名は今の
  まま `moqt` にする。npm のパッケージは `moq-wasm` から `moqt` に改名し、import は
  `moqt` と `moqt/media` になる。wasm は、ブラウザで動かす実装の手段であり、
  パッケージの中身の名前ではない。
- **名前はレジストリに公開するまで変えられる。** 今の利用者は、tarball の URL と
  import を書き換えれば移れる。リポジトリ名（`moq-wasm`）を変えるかどうかも、公開を
  決めるときに一緒に決める。GitHub は、改名後も旧 URL を転送する。
- **版は言語ごとに独立させる。** 3つの SDK は、ライブラリとしてではなく relay との
  通信でしか関わらないので、版を揃える必要が無い。各 SDK は、自分が変わったときに
  版を上げる。session がネゴシエートする draft のバージョンは、各 changelog に書く。
- semver に従い、1.0 までは minor の版で壊してよい（ブラウザ SDK と同じ）。

## 決定事項

1. **Python の wheel: Linux x86_64 と macOS arm64。** ほかのプラットフォームは、
   git source からのソースビルドで使える（動作の保証はしない）。
2. **Rust の入口: `moqt` の中の `client` module。** 新しい crate は作らない。
3. **名前: 3言語とも `moqt`。** npm の `moq-wasm` は `moqt` に改名する。

## 移行計画
1手順を1 PR にする。リファクタの PR は、それに依存する機能の PR より先に出し、依存
するものは stacked PR にする。

1. **feat(moqt): `client` module。** `SessionRoutes` の振り分けと wasm の
   `Session`・`Subscription`・`FetchStream`・`NamespaceWatch`・`TrackWriter` の
   中身を、言語に依存しない Rust の型として足す。`EventHandler` もここで足す。
   `SessionRoutes` のテストを移し、Rust でユニットテストする。利用者はまだ無い。
2. **refactor(broadcast): `RelayConnection` に `client::EventHandler` を実装し、
   `client::Session` から作れるようにする。**
3. **refactor(wasm): wasm の `Session` を `moqt::client` の binding にする。**
   `session_routes.rs` を消す。JS の API は変えない。ブラウザの unit test と E2E
   （`run-media-e2e.mjs`、`run-meeting-e2e.mjs`）で守る。
4. **refactor(python): Python の `Session` を `moqt::client` の binding にする。**
   `route_events` を消す。Python の API は変えない。pytest で守る。
5. **refactor(python): 名前を JS に合わせる。** 「Python SDK に足す API」の名前の
   変更と、`examples/python` の bot の書き換え。
6. **feat(python): 購読と取得の API。** 購読のオプション、`fetch`、`track_status`、
   `Subscription` の `done`・`groups`・`largest`・`set_forward`・`end_before_group`、
   `Object` の `subgroup_id` と `status`。
7. **feat(python): 配信の API。** `Announcement.close`、`publish_track(publish=True)`、
   `write_group`、`next_subscriber`。
8. **feat(python): トークンと `RequestError`、`CloseInfo`。**
9. **docs(moqt): Rust SDK の README と changelog。** README の例を
   `moqt::client` にし、依存の書き方を tag にする。`crates/moqt/CHANGELOG.md` を
   始める。
10. **ci: `rust-release.yml`（`rust-v*`）。**
11. **ci: `python-release.yml`（`python-v*`）。** wheel の matrix と、wheel での
    pytest。`crates/python/CHANGELOG.md` を始め、README に wheel の入れ方を書く。
12. **refactor(wasm): npm のパッケージ名を `moqt` にする。** `package.json`、
    examples の import、`check:pack`、`sdk-release.yml` の tarball の名前、
    `browser-sdk.md` の「配布」を一緒に変える。

1〜4 は重複を1か所にする部分で、5〜8 は Python の機能を JS に追いつかせる部分で
ある。9〜11 の配布は、それぞれの機能がそろってから出す。12 はほかの手順に依存しない
ので、いつ出してもよい。

各 PR では、変える component の architecture document（`moqt/architecture.md`、
`python/architecture.md`、`wasm/browser-sdk.md`）も同時に更新する。

## 範囲外

- **レジストリへの公開。** 「配布」に理由を書いた。core API が 1.0 になったときに、
  名前（crates.io の `moqt` が使えないこと、リポジトリ名）と一緒に決める。
- **Python と Rust のメディア API。** JS の `moq-wasm/media` に当たるもの。Rust は
  `broadcast` を使う。Python は、メディアを配信する bot が出たときに決める。
- **Linux aarch64 と Windows の wheel。** 必要になったら matrix に足す。Windows は
  `moqt` を動かしたことが無いので、足す前に CI で pytest を回せるようにする。
