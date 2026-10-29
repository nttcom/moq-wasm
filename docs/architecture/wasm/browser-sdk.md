# ブラウザ SDK 設計

## ステータス
承認済みで、未実装。「移行計画」の PR が入るまで、ここに書いたものは存在しない。
1.0 の範囲は「決定事項」にまとめた。この計画のゴールは、npm パッケージとして使える形に
整えるところまでで、配布は GitHub Release に添付する tarball から始め、npm への公開は
含まない（「配布」を参照）。採用後は、パッケージ構成、公開 API、ここに書いた
不変条件を変える変更で、このファイルも同時に更新する。

## 目標
Web 開発者が npm パッケージを1つ入れるだけで、MoQ のメディア PubSub を数十行で
書けるようにする。relay に接続し、カメラとマイクを配信し、他人が配信する namespace
を再生する。同じパッケージで、独自データ（チャット、テレメトリ、制御）を運ぶアプリ
向けに、型付きの track 単位 API も提供する。

SDK は `examples/browser` から切り出す。`examples/browser` は切り出し後も SDK の上で
動き続ける。現状、視聴側はこの目標に近い（`LivePlayer`）が、配信側は遠い。

| 項目 | 現状 | SDK |
| --- | --- | --- |
| 接続 | `new MoqtClientWrapper()`、`connect(url, { authToken })`。`sendClientSetup` が露出している | `connect(url, options)` が `Session` を返す |
| namespace の再生 | `new LivePlayer({ client, container })`、`start(namespace, authInfo)` | 同じ形で、`Session` の上で動く |
| メディアの配信 | `lib` には無い。meeting が `mediaPublisher.ts`（約1100行）、`meetingCatalog.ts`、`utils/media/*` の encoder と LOC、`serveSubscribes.ts` を組み合わせている | `publish(session, namespace, mediaStream)` |
| track 単位の PubSub | request id と track alias をキーにした callback、`sendSubgroupHeader` / `sendSubgroupObject`、JS が振る request id | `Subscription` / `TrackWriter` の handle と async iteration |
| パッケージ | wasm は `examples/browser/pkg` にビルド。vite alias の `@moqt`、`@player`。worker は TS ソースから `new URL(..., import.meta.url)` で読む | ビルド済みの JS、`.d.ts`、wasm、worker ファイルを含む1パッケージ |

## 目標の利用コード

接続:

```ts
import { connect } from 'moq-wasm'

const session = await connect('https://relay.example.com:4433', {
  // 関数なら、接続、GOAWAY による移動、再接続のたびに呼ばれる
  token: () => fetchTokenFromMyBackend(),
})
session.addEventListener('statechange', () => console.log(session.state))
session.closed.then((info) => console.log('session closed', info))
```

カメラとマイクの配信:

```ts
import { publish } from 'moq-wasm/media'

const stream = await navigator.mediaDevices.getUserMedia({ video: true, audio: true })
const broadcast = await publish(session, ['room-1', 'alice'], stream, {
  video: {
    codec: 'av01.0.08M.08', // 省略時は avc1.640028
    label: 'Camera',
    renditions: [
      { label: '720p', width: 1280, height: 720, bitrate: 2_500_000 },
      { label: '360p', width: 640, height: 360, bitrate: 600_000 },
    ],
    catalog: { 'com.example.role': 'camera' }, // track ごとの独自フィールド
  },
  audio: { codec: 'opus', label: 'Mic' },
  catalog: { 'com.example.room': 'room-1' }, // catalog ルートの独自フィールド
})

broadcast.replaceTrack('video', screenStream.getVideoTracks()[0])
broadcast.setMuted('audio', true)
await broadcast.close()
```

namespace の再生:

```ts
import { LivePlayer, PlayerControls } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

const container = document.querySelector<HTMLElement>('#view')!
const player = new LivePlayer({ session, container })
new PlayerControls({ player, container }) // シークバー、再生/一時停止、LIVE、音量、画質メニュー
await player.start(['room-1', 'alice'])
```

コントロールをページで作る場合は、`PlayerControls` を使わずに
`player.addEventListener('statechange', ...)` で `player.state` を描く。

メディアを使わない track 単位の PubSub:

```ts
const chat = await session.publishTrack(['room-1', 'alice'], 'chat')
await chat.writeGroup([new TextEncoder().encode('hello')])

const subscription = await session.subscribe(['room-1', 'alice'], 'chat')
for await (const object of subscription) {
  console.log(object.groupId, new TextDecoder().decode(object.payload))
}

for await (const event of session.watchNamespaces(['room-1'])) {
  if (event.kind === 'announced') player.start(event.namespace)
}
```

オプションを省いた最小の形（`publish(session, namespace, stream)`）なら、自分を配信し、
相手を再生するページが合計30行未満で書ける。

## パッケージ構成

レイヤーごとにパッケージを分けず、subpath exports を持つ npm パッケージを1つにする。

```
moq-wasm            core: init, connect, Session, Subscription, TrackWriter, 型
moq-wasm/media      publish(), MediaBroadcast, LivePlayer, PlayerControls
moq-wasm/media/player.css
moq-wasm/media/controls.css
```

1パッケージにする理由: `core` と `media` は1つの wasm インスタンスと1つの `Session`
を共有する。パッケージを2つに分けると、それぞれが wasm glue のコピーを持つ。両者の
バージョンがずれるとページに wasm インスタンスが2つでき、wasm の初期化が重なると
ヒープが壊れることがすでにわかっている。subpath exports なら tree shaking が効き、
視聴だけ、データだけのバンドルは小さく保てる。`media` はプレイヤー、decoder と
encoder の worker、CSS を引き込むが、`core` はどれも引き込まない。

ソースの置き場所: Rust 側は `crates/wasm` のまま。TypeScript 側は
`examples/browser/lib` から、その隣のパッケージディレクトリ（`crates/wasm/js/`）へ
移す。ビルドは `wasm-pack build --target web`、`tsc`、worker のバンドルで行う。
`examples/browser` は、`@moqt` と `@player` の alias ではなく、workspace のパスで
このパッケージに依存する。

何をどこへ移すか:

| 移動元 | 移動先 | 備考 |
| --- | --- | --- |
| `examples/browser/lib/moqt/loadMoqt.ts` | `core/init.ts` | single-flight な `init()`。wasm の URL を上書きできる |
| `examples/browser/lib/moqt/moqtClient.ts` | `core/session.ts`（書き直し） | 「core API」を参照。callback の wrapper は、最後の利用者が移るまで `examples/browser` に残す |
| `examples/browser/lib/msf/catalog.ts` | `media/catalog.ts` | パースだけ。組み立ては wasm へ移る |
| `examples/browser/lib/player/*` | `media/player/*` | `LivePlayer`、playout、review。テストも一緒に移す |
| `examples/browser/lib/player/ui` | `media/controls/*` | `PlayerControls`。React に依存しない DOM 部品なので、そのまま移せる |
| `examples/browser/utils/media/decoders/*`、`loc.ts`、`mseSink.ts`、jitter buffer | `media/internal/*` | export しない |
| `examples/browser/utils/media/encoders/*` | `media/internal/encoders/*` | `publish()` が使う |
| `examples/browser/lib/ui/*`（React） | `examples/browser` に残す | ページの UI |
| `utils/media/fetchReplay.ts`、`lib/moqt/subgroupWriter.ts`、`examples/media/serveSubscribes.ts` | 削除 | wasm の `broadcast` に置き換わる（下記） |

## 配信を `crates/broadcast` に寄せる

`crates/broadcast` は、すでに live-ingest、gst-plugin-moqt、onvif-ingest、Live Viewer
の MP4 publisher（`crates/wasm/src/mp4_broadcast.rs`）向けに MSF broadcast を組み
立てている。担っているのは、catalog、LOC（と CMAF）の track、media timeline、
catalog より先の PUBLISH、壁時計から始める group id、`moqt::Origin` による60秒の
FETCH キャッシュ、GOAWAY による切り替え、catalog の再送である。入力は、映像は今のところ H.264（Annex-B）だけ、音声は AAC、
Opus、A-law、MP3 を受け付ける。`mediapack` はすでに VP8/VP9/AV1 の `VideoConfig`、
LOC の demux、fMP4 の mux を持っているので、映像の codec を増やす変更は `broadcast` の
track 組み立てと catalog に閉じる（下記の変更6）。

そのため、SDK の配信側は次のように分ける。

```
JS（メインスレッド / worker）              wasm（crates/wasm -> crates/broadcast）
-------------------------                 ---------------------------------------
MediaStreamTrack
  -> MediaStreamTrackProcessor
  -> VideoEncoder / AudioEncoder
  -> EncodedVideoChunk / EncodedAudioChunk --> MediaBroadcast.pushVideo / pushAudio
     （Uint8Array に copyTo、timestamp µs）       -> BroadcastWriter::push(MediaEvent)
                                                  -> catalog、LOC、timeline、Origin、
                                                     PUBLISH、FETCH、SUBSCRIBE への応答
```

JS に残すのは、capture、encoder の設定、keyframe の要求、bitrate の制御で、どれも
ブラウザが必要なものである。プロトコルと MSF に関わる処理は、すべて wasm へ移す。
これにより、配信側で次のものが不要になる。

- `crates/wasm` の `ClientState` が持つ publisher 側の管理情報（announce した
  namespace、受けた SUBSCRIBE とその join point、配信した最大の location、subgroup
  の状態）。callback API を廃止した時点で消える。採番、キャッシュ、FETCH・
  FETCH_CANCEL・TRACK_STATUS への応答は `Origin` が、SUBSCRIBE への応答は
  `broadcast` が担う。`ClientState` の subscriber 側も callback API と一緒に消える
  （「core API」を参照）。
- JS の `SubgroupWriter`、`fetchReplay`、`serveSubscribes.ts`。group と object の
  採番、リプレイ用キャッシュ、SUBSCRIBE/FETCH への応答を、2か所ではなく Rust の
  1か所で行う。
- `meetingCatalog.ts` の catalog 組み立て。catalog は `broadcast::build_catalog`
  が作り、meeting が独自に載せている役割や bitrate は、`publish()` の独自フィールド
  （下記の変更7）で載せる。

このために、下の層で次の変更が要る。それぞれ別の PR にする。

1. **session の共有。** `RelayConnection` は `MoqtTarget` から自分で
   `moqt::Session` を開く。ページは配信と再生を1つの session で行うので、
   `broadcast` には既存の `moqt::Session` から `RelayConnection` を作る手段が要る
   （stream ledger と catalog の状態は connection が持ち、session の寿命は所有者が
   持つ）。これで `Mp4Broadcast` も2本目の WebTransport session を開かなくなる。
2. **Annex-B からの parameter set。** `Mp4Broadcast` は MP4 の avcC record を
   `broadcast` に渡している。Annex-B モードの WebCodecs encoder は SPS/PPS を
   in-band で出すので、`MediaBroadcast` は最初の keyframe から `mediapack::h264`
   （`ParameterSetTracker`、`AvcDecoderConfigurationRecord::from_parameter_sets`）で
   `VideoConfig::Avc` を作る。gst-plugin-moqt と MPEG-TS demuxer も同じことをして
   いる。encoder が解像度を変えたら作り直す。
3. **rendition。** 複数の bitrate で配信するカメラ（meeting の画質切り替え）は、
   CMSF の switching set である。`live-ingest` は、transcode した rendition を
   `GroupAlignment` を使って `Broadcast::send_object` で書いている。ブラウザでは、
   rendition ごとに別の `VideoEncoder` へ同じ `VideoFrame` を渡し、同じフレームで
   全員に keyframe を要求するので、group id は完全に揃う。`MediaBroadcast` は、
   新しい経路を作らず、既存の rendition の経路の上に `addRendition(track, label)` を
   出す。
4. **track のライフサイクル。** ミュート、ソースの差し替え、画面共有では、track が
   catalog から抜けて戻る必要がある。今の `Broadcast` は track を足すことしかできない。
   追加するのは `Broadcast::remove_track`（catalog の更新と PUBLISH_DONE）だけ。
5. **Audio Level。** meeting の話者検出は、LOC の Audio Level property
   （draft-ietf-moq-loc-04）を読む。JS が、今の `utils/media/audioLevel.ts` と同じく
   capture したサンプルからレベルを計算し、
   `pushAudio` に渡す。`broadcast` はそれをその object の property として書く。
6. **映像 codec の追加（VP8、VP9、AV1）。** `TrackPublisher` は今
   `VideoConfig::avc()` を要求する。これを `VideoConfig` の全 variant に広げ、
   catalog の `codec` には WebCodecs の `decoderConfig.codec` をそのまま使う（VP9 の
   level はビットストリームから決まらないため）。CMAF の兄弟 track は既存の fMP4 muxer
   （`vp08`/`vp09`/`av01`）で作る。ブラウザからの AAC は、WebCodecs の
   `decoderConfig.description`（AudioSpecificConfig）から `AudioConfig::Aac` を作れば、
   既存の経路にそのまま乗る。プレイヤー側は catalog の codec 文字列で WebCodecs の
   decoder を設定するので、`VideoDecoder.isConfigSupported` で弾くだけでよい。
7. **catalog の独自フィールド。** MSF は、仕様のフィールドと衝突しない独自フィールドを
   producer が足すことを認め、parser には知らないフィールドを無視することを求めている
   （draft-ietf-moq-msf-01 §5）。役割も、仕様の役割と衝突しなければ独自のものを使って
   よい（§5.1）。`Broadcast` に、track ごとと catalog ルートの独自フィールド
   （JSON のオブジェクト）を受け取る口を足し、`build_catalog` がそれを出力に混ぜる。
   MSF が定めるフィールド名と衝突するキーは、`publish()` の時点でエラーにする。
   label と role は独自フィールドではなく、MSF の `label`、`role` に入れる。
8. **CMAF の opt-in。** 今の `broadcast` は、すべての media track に `_cmaf` の兄弟
   track を作る。`SourceTracks` に CMAF を作るかどうかのフラグを足す。既定は今のまま
   にして live-ingest などは変えず、`publish()` だけ既定で切る。
   `publish(..., { cmaf: true })` で有効にする。

## Safari 対応

Safari 26 は WebTransport と WebCodecs を持つが、Chrome と違う点がいくつかある。SDK は
次のように扱う。

- **WebTransport の datagram。** Safari は今の仕様の `datagrams.createWritable()`
  だけを持つ。wasm はすでに `createWritable` を先に試している。
- **映像の capture。** `MediaStreamTrackProcessor` が使えない環境では、track を
  `<video>` に流し、`requestVideoFrameCallback` ごとに `new VideoFrame(video)` で
  フレームを取る。どちらで取るかは機能検出で決め、encoder から先は同じ経路にする。
- **音声の capture。** `MediaStreamTrackProcessor` が使えない環境では、AudioWorklet
  で PCM を取り、`AudioData` にして `AudioEncoder` に渡す。
- **encoder の設定。** 使う codec と `avc.format` は、毎回
  `VideoEncoder.isConfigSupported` / `AudioEncoder.isConfigSupported` で確かめる。
  Annex-B 出力が無い環境では avcC 形式で受け取り、wasm で
  `mediapack::h264::avcc_to_annexb` を使って Annex-B に直す。codec を指定しない
  `publish()` は、Opus を encode できない環境では AAC に切り替える。指定された codec が
  使えなければ reject する。
- **再生。** `MediaStreamTrackGenerator` が無いので、映像は canvas の sink で出す
  （`LivePlayer` はすでにそうしている）。
- **テスト。** Playwright の WebKit が WebTransport を持たない間は、Safari は実機の
  Safari での手動確認とし、確認手順を `examples/browser` の README に書く。

## `MediaBroadcast`

`MediaBroadcast`（wasm）は `Mp4Broadcast` を一般化したものである。`Session` と
namespace から作り、ソースごとに `BroadcastWriter` を1つ持つ。onvif-ingest が
カメラの profile ごとにそうしているのと同じく、`SourceTracks` で、カメラとマイクを
`video`/`audio`、画面共有を `screen`/`screen_audio` と名付ける。公開するのは
`pushVideo`、`pushAudio`、`addRendition`、`removeTrack`、`streams()`、`close()`。
JS の `publish()` は `MediaStream` をこれにつなぎ、「目標の利用コード」の
`broadcast` handle を返す。

メディアを使わない配信（チャット、データ）には `broadcast` は要らない。
`moqt::Origin` を直接使う `Session.publishTrack` で行う。catalog がすでに載せて
いる broadcast の `chat` track には、同じ `TrackWriter` 型の
`broadcast.track('chat')` で書く。

## core API

core API は、制御メッセージをそのまま出すのではなく、Rust の crate にすでにある
handle 型（`Subscriber`、`Publisher`、`Subscription`、`TrackReader`、`TrackWriter`、
`Origin`）で `moqt::Session` を包む。ルールは次のとおり。

- 公開 API に request id、track alias、subgroup id を出さない。request id は moqt
  が振り、Maximum Request ID も moqt が守る。handle は、自分がどのリクエストから
  生まれたかを知っている。
- 1リクエストに1 handle。`subscribe` は SUBSCRIBE_OK で `Subscription` に解決し、
  失敗すると draft のエラーコードを持つ `RequestError` で reject する。handle の
  メソッドは SUBSCRIBE_UPDATE と UNSUBSCRIBE で、handle を捨てる（`close()`、または
  `using` / `Symbol.asyncDispose`）と UNSUBSCRIBE する。
- データは session 全体の callback ではなく、async iteration で届く。
  `Subscription` は、subgroup stream ごとに group 順・object 順で並ぶ
  `AsyncIterable<MoqObject>` である。stream を分けて扱いたい利用者（プレイヤーは
  映像を group ごとに decode する）のために、`groups(): AsyncIterable<Group>` も
  用意し、各 `Group` は `AsyncIterable<MoqObject>` とする。背圧は iterator が担う。
  JS が次を求めたときに、wasm が次の object を読む。
- session 単位のイベントは、promise（`session.closed`）と async iterable
  （`session.watchNamespaces(prefix)`）で出す。ページ全体で1つしかない callback の
  枠は作らない。
- session の状態は `session.state`（`'connected'`、`'reconnecting'`、`'closed'`）と
  `statechange` イベントで出す。再接続中も handle は生きたままである（「再接続」を参照）。
- 境界をまたぐ値は `Uint8Array` と `bigint`。`MoqObject` は
  `{ groupId, subgroupId, objectId, payload, extensions, status }`。

形（TypeScript、抜粋）:

```ts
export function init(options?: { wasmUrl?: string | URL }): Promise<void>
export function connect(url: string, options?: ConnectOptions): Promise<Session>

export interface ConnectOptions {
  token?: string | (() => Promise<string>)       // CLIENT_SETUP の AUTHORIZATION TOKEN
  reconnect?: false | { maxDurationMs?: number } // 既定は有効。60秒で諦める
  fallback?: false | { webTransportTimeoutMs?: number } // 既定は有効。WebTransport を 5 秒待って wss:// へ
}

export interface Session extends EventTarget {
  readonly state: 'connected' | 'reconnecting' | 'closed'
  readonly transport: 'webtransport' | 'websocket'
  readonly closed: Promise<CloseInfo>
  subscribe(namespace: string[], track: string, options?: SubscribeOptions): Promise<Subscription>
  subscribeCurrentGroup(namespace: string[], track: string): Promise<Subscription>  // SUBSCRIBE + joining FETCH
  fetch(namespace: string[], track: string, range: FetchRange): Promise<AsyncIterable<MoqObject>>
  trackStatus(namespace: string[], track: string): Promise<TrackStatus>
  announce(namespace: string[]): Promise<Announcement>          // PUBLISH_NAMESPACE
  publishTrack(namespace: string[], track: string, options?: PublishOptions): Promise<TrackWriter>
  watchNamespaces(prefix: string[]): AsyncIterable<NamespaceEvent>
  close(code?: number, reason?: string): Promise<void>
}

export interface Subscription extends AsyncIterable<MoqObject>, AsyncDisposable {
  readonly largest?: Location
  groups(): AsyncIterable<Group>
  update(update: { forward?: boolean; endGroup?: bigint }): Promise<void>
  close(): Promise<void>
}

export interface TrackWriter extends AsyncDisposable {
  writeGroup(objects: Uint8Array[], options?: { extensions?: Extensions }): Promise<void>
  close(): Promise<void>                                      // PUBLISH_DONE
}
```

`subscribeCurrentGroup` と配信側の `TrackWriter` は、moqt にすでにある
`Subscriber::subscribe_current_group` と joining FETCH の範囲解決の上に作る。
`LivePlayer` の catalog 追従は `subscribeCurrentGroup(namespace, 'catalog')` に
なる。

### 認証トークン

`token` は文字列か、文字列を返す非同期関数を受け取る。関数なら、最初の接続、GOAWAY に
従った新しい relay への接続、再接続のそれぞれで、CLIENT_SETUP を送る直前に呼ぶ。期限の
短い JWT を使うページは、ここで backend から新しいトークンを取る。これは wasm ではなく
moqt の役目にする。`ClientConfig::authorization_token` を、固定値と、接続ごとに呼ばれる
provider の両方を表せる型に変え、`Session` が新しい `Connection` を開くたびに provider
を呼ぶ。Rust の publisher（live-ingest など）も同じ口を使える。

### 再接続

session は、GOAWAY が無いままの切断（ネットワークの変化、relay の停止）でも自分で
接続し直す。仕組みは GOAWAY の移行（`docs/architecture/moqt/session-migration.md`）と
同じで、`Session` が同じ URL に新しい `Connection` を開き、論理リクエストの表にある
リクエストを送り直す。アプリから見た request id、track alias、handle は変わらない。
違いは、古い接続がもう使えないことだけである。

- **購読側。** 各 `Subscription` は新しい接続で SUBSCRIBE し直し、最後に届いた
  object から relay の最大の location までを FETCH で埋める（移行時の joining FETCH
  による埋め合わせと同じ）。relay のキャッシュ（既定60秒）に残っている範囲なら、
  iterator から見た object は途切れない。キャッシュから落ちた範囲は欠け、次に届く
  object の group id が飛ぶ。
- **配信側。** `Origin` の採番とキャッシュは session に属するので、そのまま残る。
  新しい接続で PUBLISH_NAMESPACE と PUBLISH を送り直し、各 track は次の group から
  新しい接続に流れる（移行時と同じ）。catalog は、送った接続の世代が変わったので次の
  送信で送り直される。
- **再試行。** 0.5秒から始めて最大10秒まで間隔を倍にしながら、`maxDurationMs`
  （既定60秒、relay のキャッシュ保持時間に合わせる）まで試す。その間 `state` は
  `'reconnecting'` で、配信側の object は `Origin` に溜まる。諦めたら
  `session.closed` が解決し、すべての handle が終わる（iterator は終わり、書き込みは
  reject する）。
- **認証エラー。** 再接続先が SETUP で認証エラーを返したら、トークンを取り直して1回
  だけ再試行し、それでも失敗したら諦める。`reconnect: false` なら、切断で
  `session.closed` が解決する。

callback スタイルからの移行: `examples/browser` が使っている間は、wasm の
`MOQTClient` の callback export を残し、新しい `Session` は別の wasm クラスにする。
example を1つずつ別の PR で移し、最後の1つと一緒に callback のクラス、`ClientState`、
`MoqtClientWrapper` を消す。最初に移すのは `LivePlayer` である。core API の一番の
利用者であり、「L1 は alias ごとの handler だけを登録する」というルールもこれで
不要になる。

## bundler 互換

パッケージは TypeScript ではなくビルド済みの JavaScript を配る。2種類の付随
ファイルは、主要な bundler が設定なしで扱える方法で読み込む。

- **wasm**: `wasm-pack --target web` の glue は、`moqt_bg.wasm` を
  `new URL('moqt_bg.wasm', import.meta.url)` で解決する。Vite、webpack 5、Rollup
  （URL plugin 付き）、Parcel はこれを書き換える。CDN や、書き換えない bundler には
  `init({ wasmUrl })` で対応する。
- **worker**: decoder と encoder の worker は、パッケージのビルド時に単体で動く
  ファイルにバンドルし、
  `new Worker(new URL('./worker.js', import.meta.url), { type: 'module' })` で起動
  する。これを書き換えない bundler のために、オプションで `workerUrl` を上書き
  できる。
- `init()` は single-flight で、何度呼んでも同じ結果になる。`connect()` が内部で
  呼ぶ。
- パッケージに実行時の依存は無い。React は examples に残す。

## バージョニング

- パッケージのバージョンは、crate のバージョンとは独立にリポジトリのリリースに合わせ、
  semver に従う。1.0 までは minor でも API を壊してよく、壊したものはすべて
  changelog に書く。公開するまでは `package.json` の version を 0.x に保つだけで、
  changelog は公開するときに始める。
- 公開範囲は `moq-wasm` と `moq-wasm/media` が export するもので、`internal/` 以下は
  含まない。session がネゴシエートする draft のバージョンも API の約束の一部とし、
  changelog に書く。
- wasm、glue、TypeScript は1つの成果物なので、ページがそれらのバージョンを混ぜる
  ことはない。

## 移行計画
1手順を1 PR にする。リファクタの PR は、それに依存する機能の PR より先に出し、依存
するものは stacked PR にする。

1. **feat(moqt): 接続ごとに呼ばれる認証トークンの provider。** `ClientConfig` の
   `authorization_token` を、固定値と provider の両方を表す型にする。
2. **feat(moqt): GOAWAY 無しの切断からの自動再接続。** 移行の仕組みを使い、購読の
   FETCH による埋め合わせ、再試行の間隔と上限、認証エラー時のトークン取り直しを
   入れる。relay を止めて戻す Rust のテストで確かめる。
3. **refactor(broadcast): 既存の `moqt::Session` から `RelayConnection` を作る。**
   `Mp4Broadcast` はページの session を受け取る。live-ingest/onvif のコンストラクタは
   `MoqtTarget` のまま。
4. **feat(wasm): handle スタイルの subscribe、fetch、track status、namespace 監視を
   持つ `Session` wasm クラス。** `moqt::Subscriber` の上に、`MOQTClient` と並べて
   作る。Rust でユニットテストする。JS の利用者はまだ無い。
5. **feat(wasm): `moqt::Origin` の上の `TrackWriter` / `publishTrack` と
   `announce`。**
6. **refactor(browser): `LivePlayer` を新しい `Session` に載せる。** Live Viewer と
   ONVIF の example もあとに続く。ブラウザ E2E（`run-media-e2e.mjs`）で守る。
7. **feat(broadcast): Annex-B の parameter set、`remove_track`、Audio Level、
   VP8/VP9/AV1 の映像、catalog の独自フィールド、CMAF の opt-in**（PR 6本）。
8. **feat(wasm): `MediaBroadcast`**（`Mp4Broadcast` の一般化）と、
   **feat(browser): 既存の encoder を使う `publish(session, namespace, MediaStream)`。**
   media example の publisher をこれに移し、その経路から `serveSubscribes.ts`、
   `SubgroupWriter`、`fetchReplay` を消す。
9. **feat(browser): `publish()` に rendition、画面共有、codec の選択、catalog の
   独自フィールドを足す。** そのあと meeting
   example を移し、`mediaPublisher.ts` のプロトコル部分と `meetingCatalog.ts` の
   catalog 組み立てを消す。
10. **feat(browser): Safari 対応。** capture の代替経路、encoder 設定の機能検出、
    avcC から Annex-B への変換、AAC への切り替え。実機の Safari で配信と再生を
    確かめる。
11. **refactor: TS をパッケージディレクトリへ移す。** subpath exports、worker の
    バンドル、`examples/browser` を workspace 依存にする。CI に、新しい Vite
    プロジェクトが `npm pack` した tarball でビルドできるかの確認を足す。
12. **refactor(wasm): `MOQTClient`、`ClientState`、`MoqtClientWrapper` を消す。**
    使う example が無くなってから行う。

12 が入った時点で、パッケージは完成とする。配布は「配布」の手順で行う。

## 決定事項
1.0 の範囲として決めたもの。

1. **codec: 映像は H.264、VP8、VP9、AV1、音声は Opus と AAC。** 既定は
   `avc1.640028` と `opus`。`publish()` の `codec` で選び、ブラウザが encode できない
   codec を指定したら reject する（「配信を `crates/broadcast` に寄せる」の変更6）。
2. **認証トークン: 文字列か、更新用の非同期関数。** 関数は、接続、GOAWAY による移動、
   再接続のたびに呼ぶ（「core API」の「認証トークン」）。
3. **再接続: 自動。** GOAWAY 無しの切断でも session が接続し直し、handle はそのまま
   使える。relay のキャッシュにある範囲は FETCH で埋め、上限（既定60秒）を過ぎたら
   諦める（「core API」の「再接続」）。
4. **catalog: `publish()` で指定できる。** track ごとの label、role、rendition と、
   track ごと・catalog ルートの独自フィールドを渡せる。MSF のフィールド名と衝突する
   キーはエラーにする（変更7）。
5. **Audio Level: 含める。** `publish()` は音声の object ごとにレベルを計算し、LOC の
   Audio Level property として送る（変更5）。
6. **CMAF の兄弟 track: opt-in。** `publish()` は既定で CMAF を作らず、
   `{ cmaf: true }` で作る。他の publisher（live-ingest など）の既定は変えない
   （変更8）。
7. **パッケージ名: `moq-wasm`。** `package.json` の `name` と import パスに使う。
   npm で空いているかは、公開を決めたときに確かめる。
8. **対応ブラウザ: Chrome、Edge、Safari。** Safari の違いは「Safari 対応」に書いた
   代替経路で吸収する。
9. **プレイヤー UI: `PlayerControls` も含める。** 今の `PlayerControls` は、
   コンストラクタで container、player、ログ用の callback を受け取り、ページが
   `onStateChange` から `render()` を呼ぶ形である。SDK では次のように変える。
   - 引数を `{ player, container, onLog? }` のオプションオブジェクトにする。
     `onLog` は省略できる。
   - `LivePlayer` を `EventTarget` にして `statechange` イベントを出し、
     `PlayerControls` はそれを自分で購読して描き直す。ページが `render()` を呼ぶ必要は
     無くなり、ページ自身も同じイベントを購読できる（`onStateChange` の枠を
     取り合わない）。
   - 見た目は `controls.css` に閉じ、class 名と CSS 変数（色、サイズ）を公開範囲に
     含める。markup の構造は含めない。
   - `dispose()` で、追加した DOM とイベントの購読を外す。
   移す PR は移行計画の手順6（`LivePlayer` を新しい `Session` に載せる）のあとに
   出す。
10. **安定性: semver。1.0 までは minor で壊してよい。** core API（`Session`、
    `Subscription`、`TrackWriter`）を先に 1.0 にする。`moq-wasm/media` は、examples
    以外のアプリが1つ、それで配信と再生をするまで 1.0 未満のままにする。両方を
    1パッケージで出すので、media API が 1.0 未満であることは、バージョンを分けず、
    doc comment と README に書く。

## 配布

最初は npm レジストリに公開せず、`npm pack` で作った tarball を GitHub Release に添付して
配る。API を壊してもよい間（1.0 未満）は、試してくれる利用者と直接やり取りしながら進める
ので、レジストリの利点（検索、範囲指定による更新、CDN）はまだ要らない。

利用者は、`package.json` に tarball の URL を書いて `npm install` する。中身はビルド
済みなので、Rust や wasm-pack は要らない。lock ファイルには URL とハッシュが記録される。

```json
{
  "dependencies": {
    "moq-wasm": "https://github.com/<owner>/<repo>/releases/download/sdk-v0.1.0/moq-wasm-0.1.0.tgz"
  }
}
```

リリースの手順:

1. パッケージの `package.json` の version を上げる。
2. `sdk-v<version>` の tag を push する。crate とは別に版を刻むので、tag には `sdk-`
   を付ける。
3. CI がその tag で wasm と TS をビルドし、`npm pack` の tarball を作り、同じ名前の
   GitHub Release に添付する。changelog の該当部分を Release の本文にする。

移行計画の手順11の CI（`npm pack` の tarball で Vite プロジェクトをビルドする確認）が
3 のビルドを兼ねる。この Release の workflow は手順12のあとの PR で足す。

tarball 配布の制約: `^0.1.0` のような範囲指定、`npm update`、Dependabot や Renovate に
よる更新は効かず、利用者は URL を書き換えて更新する。jsDelivr や unpkg からも使えない。

## 範囲外

- **npm への公開。** core API が 1.0 になったときに改めて決める。それまでに名前
  （`moq-wasm`）が npm で空いているかを確かめる。採らなかった配布方法は次のとおり。
  - **Git の URL（`github:<owner>/<repo>#<tag>`）。** npm はリポジトリのルートの
    `package.json` しか見ないので、サブディレクトリ（`crates/wasm/js/`）にある
    パッケージは入れられない。入れられたとしても、インストール時の `prepare` で
    wasm のビルドが走り、利用者に Rust と wasm-pack が要る。
  - **GitHub Packages。** public なパッケージでも、インストールに認証トークンが要る。
