# ブラウザ SDK 設計

## ステータス
承認済み。「移行計画」の step 1〜12 を実装した。
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
| 接続 | `MoqSession.connect(url, { token })` | `connect(url, options)` が `Session` を返す |
| namespace の再生 | `moq-wasm/media` の `new LivePlayer({ container, callbacks })`、`start(session, namespace)` | 同じ形で、`Session` の上で動く |
| メディアの配信 | `moq-wasm/media` の `publish(session, namespace, mediaStream)`。meeting の `mediaPublisher.ts` もこれで配信する | `publish(session, namespace, mediaStream)` |
| track 単位の PubSub | `MoqSession` の `subscribe` / `fetch` / `publishTrack` が返す `MoqSubscription` / `MoqFetch` / `TrackWriter` の handle と async iteration | `Subscription` / `TrackWriter` の handle と async iteration |
| パッケージ | `crates/wasm/js` の `moq-wasm`（npm workspace）。examples は `source` condition で TS ソースをそのまま読む。`npm run build` が `dist`（JS、worker、wasm）と `types`（`.d.ts`）を作り、CI が `npm pack` の tarball を新しい Vite プロジェクトでビルドして確かめる | ビルド済みの JS、`.d.ts`、wasm、worker ファイルを含む1パッケージ |

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
  },
  audio: { codec: 'opus', label: 'Mic' },
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

for await (const event of await session.watchNamespaces(['room-1'])) {
  if (event.kind === 'announced') player.start(event.namespace)
}
```

オプションを省いた最小の形（`publish(session, namespace, stream)`）なら、自分を配信し、
相手を再生するページが合計30行未満で書ける。

## パッケージ構成

レイヤーごとにパッケージを分けず、subpath exports を持つ npm パッケージを1つにする。

```
moq-wasm            core: init, connect, Session, Subscription, TrackWriter, 型
moq-wasm/media      publish(), MediaBroadcast, LivePlayer, PlayerControls, parseCatalog, Mp4Broadcast
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
| `examples/browser/lib/moqt/moqtClient.ts` | `core/session.ts`（書き直し） | 「core API」を参照 |
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
  の状態）。callback API と一緒に消した。採番、キャッシュ、FETCH・
  FETCH_CANCEL・TRACK_STATUS への応答は `Origin` が、SUBSCRIBE への応答は
  `broadcast` が担う。`ClientState` の subscriber 側も callback API と一緒に消した
  （「core API」を参照）。
- JS の `SubgroupWriter`、`fetchReplay`、`serveSubscribes.ts`。group と object の
  採番、リプレイ用キャッシュ、SUBSCRIBE/FETCH への応答を、2か所ではなく Rust の
  1か所で行う。
- `meetingCatalog.ts` の catalog 組み立て。catalog は `broadcast::build_catalog`
  が作る。meeting が載せている役割と bitrate は MSF の `role` と `bitrate` で、
  独自フィールドは要らない（下記の変更7）。

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
   全員に keyframe を要求するので、group id は完全に揃う。live-ingest の経路は
   `broadcast` の `RenditionTrack` に移し、`MediaBroadcast` はその上に
   `addRendition(name, label, bitrate)` を出す。encoder の出力は順不同で届くので、
   ブラウザの rendition は `RenditionKeyframes::SameFrames` で、source より先に
   来た keyframe でもその時刻の group id を割り当てる。
4. **track のライフサイクル。** ミュート、ソースの差し替え、画面共有では、track が
   catalog から抜けて戻る必要がある。今の `Broadcast` は track を足すことしかできない。
   追加するのは `Broadcast::remove_track`（catalog の更新と PUBLISH_DONE）だけ。
5. **Audio Level。** meeting の話者検出は、LOC の Audio Level property
   （draft-ietf-moq-loc-04）を読む。JS が、今の `media/internal/audioLevel.ts` と同じく
   capture したサンプルからレベルを計算し、
   `pushAudio` に渡す。`broadcast` はそれをその object の property として書く。
6. **映像 codec の追加（VP8、VP9、AV1）。** `TrackPublisher` は今
   `VideoConfig::avc()` を要求する。これを `VideoConfig` の全 variant に広げ、
   catalog の `codec` はビットストリームから読んだ config で作る。VP9 の level は
   ビットストリームに無いので、CMAF の vpcC box と同じく画面サイズに収まる最小の
   level にし、catalog と init segment の codec を揃える。CMAF の兄弟 track は既存の
   fMP4 muxer（`vp08`/`vp09`/`av01`）で作る。ブラウザからの AAC は、WebCodecs の
   `decoderConfig.description`（AudioSpecificConfig）から `AudioConfig::Aac` を作れば、
   既存の経路にそのまま乗る。プレイヤー側は catalog の codec 文字列で WebCodecs の
   decoder を設定するので、`VideoDecoder.isConfigSupported` で弾くだけでよい。
7. **catalog の独自フィールド（見送り）。** MSF は、仕様のフィールドと衝突しない独自
   フィールドを producer が足すことを認め（MAY）、parser には知らないフィールドを
   無視することを求めている（MUST、draft-ietf-moq-msf-01 §5）。parser 側の要件は
   msf の読み込みがすでに満たしている。producer 側は、仕様のフィールドで表せない
   情報（参加者の表示名など）を載せる用途が出るまで足さない。足すときは、MSF の
   フィールド名と衝突するキーをエラーにする（MUST NOT）。label と role は独自
   フィールドではなく、MSF の `label`、`role` に入れる。
8. **CMAF の opt-in。** 今の `broadcast` は、すべての media track に `_cmaf` の兄弟
   track を作る。`SourceTracks` に CMAF を作るかどうかのフラグを足す。既定は今のまま
   にして live-ingest などは変えず、`publish()` だけ既定で切る。
   `publish(..., { cmaf: true })` で有効にする。

## Safari 対応

Safari は WebTransport と WebCodecs を持つが、Chrome と違う点がいくつかある。Safari 27.0.1
で確かめた違いと、SDK の扱いは次のとおり。

- **WebTransport の datagram。** Safari は今の仕様の `datagrams.createWritable()`
  だけを持つ。wasm はすでに `createWritable` を先に試している。
- **映像の capture。** `MediaStreamTrackProcessor` は worker の中にしか無い。page に
  無いときは、track の clone を encoder の worker へ transfer して、worker の中で読む。
  止めるときは worker が clone を止めて閉じる（`terminate` では clone が残る）。
- **音声の capture。** worker の `MediaStreamTrackProcessor` は映像の track しか読めない。
  page に無いときは AudioWorklet で PCM を 10 ms ずつ取り、`MessageChannel` で encoder の
  worker に送って `AudioData` にする（`media/internal/encoders/pcmCapture.ts`）。10 ms は、
  encoder の出力を入力の capture 時刻と突き合わせる `CaptureTimestamps` の間隔に合わせた。
- **encoder の設定。** Safari は H.264 の Annex-B 出力と Opus の encode を持つので、avcC
  から Annex-B への変換と AAC への切り替えは要らない。
- **再生。** `MediaStreamTrackGenerator` が無いので、映像は canvas の sink で出す
  （`LivePlayer` はすでにそうしている）。
- **テスト。** Playwright の WebKit は WebTransport を持たないので、Safari は実機で
  確かめる。手順は `examples/browser` の README にある。

## `MediaBroadcast`

`MediaBroadcast`（wasm）は `Mp4Broadcast` を一般化したものである。`Session` と
namespace から作り、`addSource` でソースごとに `MediaSource` を作る。
`MediaSource` は `BroadcastWriter` を1つ持ち、onvif-ingest がカメラの profile
ごとにそうしているのと同じく、`SourceTracks` で、カメラとマイクを `video`/`audio`、
画面共有を `screen`/`screen_audio` と名付ける。どのソースの track も、broadcast の
1つの catalog に載る。`MediaSource` が公開するのは `configureVideo`・`pushVideo`、
`configureAudio`・`pushAudio`、`addRendition`・`configureRendition`・
`pushRenditionVideo`、`removeTrack`。JS の `publish()` は
`MediaStream` をカメラのソースにつなぎ、画面共有は `addSource` で足す。

メディアを使わない配信（チャット、データ）には `broadcast` は要らない。
`moqt::Origin` を直接使う `Session.publishTrack` で行う。catalog がすでに載せて
いる broadcast の `chat` track には、同じ `TrackWriter` 型の
`broadcast.track('chat')` で書く。

## 会議の再生を `LivePlayer` に載せる

### 現状
meeting example は `LivePlayer` を使わず、`LocLive`、`AdaptiveBitrate`、`RenditionSplice`、
`pictureElements`、`readLocHeader` を内部パスから組み合わせて、メンバーとソース（カメラ、
画面共有）ごとに再生している。catalog の追従、rendition の切り替え、ABR は `LivePlayer`
にもあり、同じ仕組みが2つある。

### 方針
メンバーのソースごとに `LivePlayer` を1つ作り、会議が要る機能を `LivePlayer` に足す。
`LocLive` などの部品は公開しない。公開すると仕組みが2つ残り、内部の API が公開範囲に入る。

1つの session に `LivePlayer` を複数載せる形は、remote-monitoring がすでに使っている。
タイルは player に container の要素を渡し、React はその要素を置く場所を変えるだけにする。

### 足す機能

| 機能 | API | meeting での使い方 |
| --- | --- | --- |
| ソースの選択 | `LivePlayerOptions.source`（`{ video, audio? }`）。名前の映像 track と、その altGroup（draft-ietf-moq-msf-01 §5.2.12）の rendition、名前の音声 track だけを購読する。省略時は今と同じ | カメラと画面共有で player を分ける |
| 表示の高さで ABR の上限を決める | 自動選択のとき、container の高さを監視し、ladder をその高さを満たす rendition までに絞る（今の `capLadderToHeight`） | 小さいタイルは低い rendition を受ける |
| Audio Level | `LivePlayerCallbacks.onAudioLevel?(level: AudioLevel)`。音声 object の LOC Audio Level property（draft-ietf-moq-loc-04 §2.3.3.2）を渡す。`AudioLevel` を export する | 話者の検出（`dominantSpeaker.ts`） |
| 最初の映像を joining FETCH で取る | 映像の最初の購読は `subscribeCurrentGroup` にする。rendition の切り替えは今と同じく次の group を待つ | 静止した画面共有も、参加した直後に映る |
| 音声を映像より優先する | 音声の SUBSCRIBE の subscriber priority を映像より高くする。オプションにはしない | meeting の映像128、音声0 と同じ |

ソースは映像と音声の track 名で渡す。rendition は、catalog が switching set ごとに
付ける altGroup から見つかる。映像だけがミュートされて catalog から外れても、音声は名前で
見つかるので再生を続けられる。catalog の render group（§5.2.11）で映像と音声を結び付ける
方法は採らない。番号に意味が無いので、映像 track が catalog に無いときに、どの音声が
そのソースのものかを決められない。track 名は `publish()` の命名（`video` と `audio`、
`addSource` の `<name>` と `<name>_audio`）をアプリが渡し、player は命名の規則を持たない。
`source` を `start` の引数でなく player のオプションにするのは、`start` の3つ目の引数が
すでに `LiveTracks` で、meeting はソースごとに player を作るため。

### 移行の手順
1手順を1 PR にする。

1. **feat(wasm): `LivePlayerOptions.source`。**
2. **feat(wasm): 表示の高さによる ABR の上限。**
3. **feat(wasm): `onAudioLevel`。**
4. **feat(wasm): 映像の最初の購読を joining FETCH にする。**
5. **feat(wasm): 音声の subscriber priority を映像より高くする。**
6. **refactor(browser): meeting の再生を `LivePlayer` に移す。** `mediaSubscriber.ts` と、
   `localSession.ts` の映像・音声の購読と切り替え、`meetingCatalog.ts` の解析を消す。
   ブラウザ E2E（`run-meeting-e2e.mjs`）で守る。
7. **refactor(wasm): `./media/*` の export を消す。** examples が内部パスを使わなく
   なってから行う。

## 録画を `LivePlayer` で再生する

ステータス: 提案。議論の途中で、未決事項が残っている。

### 現状
`examples/browser/examples/recording-player` は、`LivePlayer` を使わない独自の再生エンジンを持つ。
FETCH の範囲、時刻から位置への対応、track の一覧を、recorder の `POST /recordings/{id}/publish`
の応答（JSON の `tracks[].chunks[]`）から得る。この JSON は recorder の保存単位（約 4 MiB の
チャンク）に結び付いた独自の形式で、ライブの `LivePlayer` にはこの入口がない。

`LivePlayer` に replay の namespace を渡しても再生できない。

- catalog を joining FETCH 付きで購読するが、replay は joining FETCH を断り、SUBSCRIBE では
  Object を送らないので、catalog が届かない。
- 巻き戻しの時間軸は、ライブで観測した group から作る（`GroupTimeline`、60 秒で失効）。
  見たことのない group へはシークできない。`MediaTimeline` は観測した group に時刻を
  付けるのにしか使っていない。
- 終わりの状態が無い。publisher の終了は「publisher が去った、戻るのを待つ」と扱う。
- 時計と AudioContext は player ごとにあり、複数の player を1つの再生位置に揃えられない。

### 方針
MoQ では、過去の Object も FETCH で取れるので、録画とライブの違いは「publisher がまだ
配信しているか」だけになる。録画を見るときは、元の publisher の代わりに recorder が
保存した Object を配信する（replay）。そこで、録画を **MSF の VOD broadcast** として
replay し、`LivePlayer` は catalog が示すとおりに再生する。

- replay の catalog は、各 track を `isLive: false`（draft-ietf-moq-msf-01 §5.2.7）にし、
  `trackDuration`（§5.2.35）を付け、catalog 全体を `isComplete: true`（§5.1.3）にする。
- 時刻から位置への対応は、media timeline track（§7）で示す。MoQ は Object を時刻でなく
  位置（Group ID と Object ID）で指定し、Group ID も group の長さも時刻と結び付かないので、
  timeline が無いと、見たことのない group がいつのものかわからない。§7.3 は、独立した
  timeline に「その時点でアクセスできる全記録」を含めると定める。ライブではそれが relay の
  キャッシュにある範囲、録画では全期間になる。§7 は用途にライブの巻き戻しと VOD での
  ランダムアクセスを挙げている。
- どのメディアにも timeline を付ける。ライブでは publisher（`crates/broadcast`）が、
  録画では replay が出す。
- `LivePlayer` は、ライブか録画かを独自の情報で分けない。catalog の `isLive` と
  timeline で振る舞いを決める。
- recorder と player は、track の名前の規則に頼らない。ソースは `renderGroup`（§5.2.11）で、
  rendition は `altGroup`（§5.2.12）で見分ける。カメラか画面共有かを示す独自のフィールドは
  足さない。表示には `label` を使う。
- recorder の HTTP API（再 publish の依頼と lease）は `LivePlayer` に入れない。SDK の
  別の subpath（`moq-wasm/recording`）に置き、`LivePlayer` とライブだけ使うアプリの
  バンドルには入れない。

### publisher（`crates/broadcast`）に要る変更

| 項目 | 今 | 変更後 |
| --- | --- | --- |
| timeline を出すソース | 名前が `video` の track（カメラ）だけ | すべてのソース。ソースごとに timeline track を1本出し、`depends` にそのソースの映像、rendition、音声を並べる。track 名の付け方は手順 1 で決める |
| entry を書く時 | `video` のキーフレームごと | ソースの group が始まるごと。映像があるときはキーフレームで、無いとき（ミュート中、音声だけのソース）は音声が始める group で書く。1つのソースの映像と音声は同じ Group ID の列を使うので、1本で両方の位置を表せる |
| group ID が飛んだとき | 一覧を空にする（映像をミュートすると起きる） | 音声の group でも entry を書くので、ID は飛ばない。一覧を空にするのは publisher が配信をやり直したときだけにする |

保持する範囲は今と同じく relay のキャッシュの保持時間（60 秒）とする。

### recorder（replay）に要る変更

| 項目 | 今 | 変更後 |
| --- | --- | --- |
| track の選び方 | 画面共有を名前（`screenshare` で始まる）で見分ける。チャットは `chat` と決め打ち | catalog の `renderGroup` ごとに1ソースとして録画し、`altGroup` の中から1本を選ぶ。チャットは `eventtimeline` の track を catalog から選ぶ |
| catalog | 元の catalog の Object をそのまま流す。track の `namespace` 欄は元の publisher のもの | replay の namespace ごとに、保存した track を並べた catalog を1つ作って流す。各 track は元の catalog から codec、幅、高さなどを引き継ぎ、`isLive: false` と `trackDuration` を付ける。catalog 全体は `isComplete: true` にし、`generatedAt` は付けない（§5.1.2） |
| 時刻から位置 | HTTP の JSON（`chunks[].capture_start_us` など）。元の timeline は保存しない | 元の publisher の timeline track もほかの track と同じく録画する。replay は、保存した timeline の Object の entry を位置ごとにまとめ、保存した group を指すものだけを残して、ソースごとに全期間の media timeline を1つ出す。元の Object は1つに直近 60 秒分しか持たないので、そのまま流さない。presentation time は元の publisher の値をそのまま使う（§7.1.1） |
| joining FETCH | 断る | 受ける。catalog と timeline は1 group なので、joining FETCH で全体が届く |
| 終わり | FETCH_OK の `end_of_track` だけ | catalog の `isLive: false` で示す。SUBSCRIBE は今どおり Object を送らずに続け、lease が切れたら session を閉じる。PUBLISH_DONE は送らない |
| 参加者の名前 | namespace に無い（`<room>/recordings/<id>/<member-index>/<content>/<session-index>`） | 元の namespace の最後の要素（参加者名）を `<member-index>` の代わりに入れる |
| transcript | transcriber は catalog を出さないので、catalog が無い | transcriber の namespace にも catalog を作り、`transcript` を event timeline として載せる |
| HTTP の応答 | `tracks` と `chunks` を含む | replay の prefix（`namespace_prefix`）と `renew_after_seconds` だけにする |

録画中に timeline の Object を取りこぼしても、次の Object が直近 60 秒分を持つので、
60 秒より短い抜けは埋まる。それより長く届かなかった区間には entry が無く、その区間へは
手前の entry からシークする。

replay が作る catalog と timeline は、保存したデータだけから決める。draft-ietf-moq-transport-14
§2.1 は、Object がどこから取っても同じバイト列で、中身が時間で変わらないことを求める
（MUST NOT change）。同じ録画の replay は Cloud Run の別のインスタンスでも立ち、同じ
namespace を PUBLISH_NAMESPACE する。relay はその全 publisher から取り込み、重複を1つに
まとめる（§8.2、relay の architecture.md の Ingress）。作った時刻やインスタンスごとの値を
入れると、同じ位置に別の中身が並ぶ。

### SDK に要る変更

`LivePlayer`:

- catalog の track が `isLive: false` のとき、シークバーの範囲を timeline の最初から最後
  （`trackDuration`）にし、`goLive` を出さない。再生位置が終わりに着いたら止まり、
  `state.ended` を立てる。
- 見たことのない group へも、timeline の entry を使ってシークする。FETCH の窓は今の巻き戻しと
  同じ（group の先頭から、4 group ずつ）。ライブでも、timeline があればこれを使い、relay の
  キャッシュに無い範囲は今どおり `FetchFailure` で扱う。
- 複数の player で時計を共有する `PlaybackGroup` を足す。group は1つの AudioContext と、
  wallclock（LOC の capture timestamp）で表した1つの再生位置を持ち、`play`、`pause`、
  `seek` を全 player に当てる。player は `LivePlayerOptions.group` で group に入る。

`moq-wasm/recording`:

- `openRecording(session, recorderUrl, id, { token })` が、`POST /recordings/{id}/publish`
  の応答を開いたまま持ち、replay の prefix を返す。`close()` で応答を閉じる。
- `renew_after_seconds` ごとに次の応答を開き、session に進行中の FETCH が無くなってから
  前の応答を閉じる。FETCH が進行中かどうかは player ではなく session の状態なので、
  `Session` にそれを知らせる手段を足す。会議の録画は player を複数使うので、lease を
  player に結び付けない。

会議の録画を再生するアプリは、`openRecording` で prefix を得て、`watchNamespaces(prefix)`
で参加者ごとの namespace を知り、ソースごとに `LivePlayer` を作って1つの `PlaybackGroup`
に入れる。チャットと字幕は、catalog の event timeline track をアプリが FETCH して並べる。

### 移行の手順
publisher の timeline から始め、recorder の replay を VOD にしてから、録画の再生を
`LivePlayer` に移す。手順 2 はライブの巻き戻しだけに効き、手順 1 の後ならいつでもよい。
1手順を1 PR にする。後方互換は持たない。手順 4 で replay の HTTP 応答と catalog が変わり、
今の recording-player は再生できなくなる。手順 4 で recorder の再生 E2E
（`recorder-archive-e2e` の再生部分、`recorder-mp4-replay-e2e`）を外し、手順 9 で
`LivePlayer` 版として戻す。

1. **feat(broadcast): すべてのソースに media timeline を出す。** entry をソースの group の
   始まりごとに書く。
2. **feat(wasm): ライブの巻き戻しで、見たことのない group へも timeline でシークする。**
3. **refactor(recorder): track を `renderGroup` と `altGroup` で選ぶ。** 名前の規則を消す。
4. **feat(recorder): replay を MSF の VOD broadcast にする。** timeline を録画し、VOD の
   catalog と全期間の timeline を出し、joining FETCH を受け、HTTP の応答を prefix と
   `renew_after_seconds` だけにする。MP4 で保存した録画の replay（`mp4_replay`）は消す。
5. **feat(recorder): replay の namespace に参加者名を入れ、transcriber の catalog を作る。**
6. **feat(wasm): `isLive: false` の catalog を、timeline を使って最後まで再生する。**
7. **feat(wasm): `PlaybackGroup`。**
8. **feat(wasm): `moq-wasm/recording` の `openRecording`。**
9. **refactor(browser): recording-player を `LivePlayer` に移す。** 独自の再生エンジンを消し、
   手順 4 で外した E2E を戻す。

### 範囲外
- MP4 で保存した録画の replay。MP4 には元の catalog も timeline も残らないので、VOD の
  broadcast にするには、サンプルの時刻から catalog と timeline を作る経路を MoQ 形式とは別に
  持つことになる。手順 4 で replay ごと消す。MP4 の録画ファイルと合成はそのまま残る。
- 録画の途中での codec や設定の変化。今の recording-player と同じく、再生を断る。
- timeline を録画していない録画（手順 4 より前の録画と、timeline を出さない publisher の録画）。
  後方互換は持たず、`LivePlayer` では再生しない。
- 録画の途中で選ばれた rendition が変わった track の切り替え。別の track として並ぶ。
- カメラか画面共有かを catalog で示すこと。今は `renderGroup` と `label` で足りる。
  画面共有だけ高い解像度で録画するなど、種類で振る舞いを変える必要が出たときに決める。

### 未決事項
- 参加者が入り直して session-index が分かれたとき、1人の参加者を時間ごとに別の namespace
  で再生することになる。`PlaybackGroup` が、ある時刻に該当する player だけを鳴らすか。
- `PlaybackGroup` を、ライブ会議の巻き戻しにも使うか。
- `Session` が進行中の FETCH を知らせる手段の形（イベントか、Promise か）。
- recorder は今、画面共有だけ高さの上限を 720p にしている（カメラは 480p）。`renderGroup`
  で選ぶようにしたとき、すべてのソースに同じ上限を使ってよいか。
- 同じ録画の replay が複数のインスタンスで立つことを許すか。許さないなら recorder を
  1 インスタンス（`maxScale: 1`）にする。

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
- wasm 内の `tracing` のログは `init({ onLog, logLevel })` で受け取る。`onLog` は
  `logLevel`（既定は `'warn'`）以上の event ごとに `LogRecord`
  （`{ timeMs, level, target, message, fields, traceId?, spanId? }`）で呼ばれる。`onLog` を渡さなければ
  ログは捨てる。`onSpan` も渡していれば、span の中で書かれたログには
  その span の `traceId` と `spanId` が付く。wasm の subscriber はページ全体で1つなので、session の option では
  なく `init` の option にし、`onLog` を付けた `init` を2回呼ぶと reject する。
  subscriber は `tracing-subscriber` の registry に自前の `Layer` を載せたもので、
  `crates/wasm/src/log_sink.rs` にある（`tracing-subscriber.md` を参照）。
- `init({ onSpan })` を渡すと、閉じた INFO 以上の span（request span
  `moqt.subscriber.subscribe` など）が `SpanRecord`
  （`{ traceId, spanId, parentSpanId?, name, startTimeMs, endTimeMs, fields }`、id は小文字 hex）で
  渡る。`logLevel` は span には効かない。span を記録する `Layer` は
  `crates/client-telemetry/src/span_record.rs` にある。
- `onLog` と `onSpan` は、レコードが属する session のトークンを第2引数で受け取る
  （provider なら最後に返した値）。session に属さないレコードと、トークンのない
  session のレコードでは `undefined`。テレメトリのレポートの `token` に入れるためのもので、
  レコードの `fields` には入れない（ClickHouse にそのまま残る）。アプリは何もしなくてよい。
  - 仕組み: `MoqSession.connect` ごとに wasm が `client-telemetry` の scope（番号）を1つ
    割り当て、トークンを scope に結び付ける。session と、それが返すオブジェクト
    （`Subscription`、`MediaBroadcast` など）のメソッドは、その scope の中で poll される。
    moqt の browser 用 `executor::spawn` は `set_spawn_wrapper` で、spawn した側の scope を
    タスクに引き継ぐので、session のタスクと broadcast のタスクも同じ scope になる。
    span は親の scope を継ぐ。
  - session を持たない `Mp4Broadcast`（トークンなしで自分で接続する）や、
    scope の外で生まれたレコードは、トークンなしで渡る。
- `init({ telemetry: { url, page } })` を渡すと、SDK が自分で wasm のログ（`logLevel` 以上）と
  span を observability サーバの `/api/client-telemetry` に送る（`core/telemetry.ts` の
  `ClientTelemetry`）。10秒ごとと `pagehide` のとき（`sendBeacon`）に、session のトークンごとに
  1レポートにまとめる。401 が返ったトークンのレコードは、以後トークンなしで送る。アプリ自身の
  ログは送らない。`onLog` と `onSpan` も渡せば両方に届く。
  - 送り先を OTel Collector ではなく observability サーバにするのは、Collector が VPC の中でしか
    待ち受けず、ブラウザに Collector の資格情報を置けないから。トークンの検証、`app_id` の付与、
    レート制限はサーバが受け持つ（`docs/architecture/observability/architecture.md` の
    「Client telemetry」）。
- SDK の TypeScript 層のログも `onLog`（と `telemetry`）に届く（`core/log.ts`）。`sdkLog` は
  コンソールに書いたうえで、`init` が置いた sink に `LogRecord` で渡す。`target` は
  `moq-wasm::media::live_player` のようなモジュール名、トークンは常に `undefined`。
  - worker（エンコーダとデコーダ）には sink がないので、`workerLog` が `{ type: 'log', … }` を
    postMessage し、ページ側の `onmessage` が `receiveWorkerLog` で `sdkLog` に渡す。
  - `LivePlayerCallbacks.onLog` と `PlayerControls` の `onLog` に渡すログは、ページが自分で
    表示するので、`forwardSdkLog` でコンソールに書かずに sink にだけ渡す。

形（TypeScript、抜粋）:

```ts
export function init(options?: {
  wasmUrl?: string | URL
  onLog?: (record: LogRecord, token: string | undefined) => void
  logLevel?: LogLevel                                          // 既定は 'warn'
  onSpan?: (record: SpanRecord, token: string | undefined) => void
  telemetry?: { url: string; page: string }                   // observability サーバへ送る
}): Promise<void>
export function connect(url: string, options?: ConnectOptions): Promise<Session>

export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace'
export interface LogRecord {
  timeMs: number                                               // Date.now() の時刻
  level: LogLevel
  target: string                                               // Rust の module path
  message: string
  fields: Record<string, string | number | boolean>
  traceId?: string                                             // onSpan があり、span の中で書かれたとき
  spanId?: string
}

export interface ConnectOptions {
  token?: string | (() => Promise<string>)       // CLIENT_SETUP の AUTHORIZATION TOKEN
  reconnect?: false | { maxDurationMs?: number } // 既定は有効。60秒で諦める
  fallback?: false | { webTransportHeadStartMs?: number } // 既定は有効。WebTransport に 500 ms 先行させて wss:// と並走
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
  watchNamespaces(prefix: string[]): Promise<NamespaceWatch>      // SUBSCRIBE_NAMESPACE; AsyncIterable<NamespaceEvent>
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

wasm 側の実装は `crates/wasm/src/session.rs` の `Session`、`Subscription`、
`FetchStream`、`NamespaceWatch` で、iterator は `next()` の Promise として出す。
JS の `AsyncIterable` への包み直しと `groups()` は、最初の利用者である `LivePlayer` を
載せる手順で足す。session のイベントは `crates/wasm/src/session_routes.rs` の
`SessionRoutes` が振り分ける。配信側のリクエスト（SUBSCRIBE、UNSUBSCRIBE、FETCH、
FETCH_CANCEL、TRACK_STATUS）は session の `moqt::Origin` へ、PUBLISH_NAMESPACE は
`moqt::namespace::NamespaceWatchers` へ、PUBLISH_DONE は該当する `Subscription` の
`done` へ送る。`announce` と `publishTrack` が返す `Announcement` と `TrackWriter` は、
その `Origin` に書く。どこも引き受けないイベントには `SessionRoutes` 自身が答える。
SUBSCRIBE は TRACK_DOES_NOT_EXIST、PUBLISH は UNINTERESTED で断り、SUBSCRIBE_NAMESPACE
には OK を返す。それぞれ `wasm.subscribe`、`wasm.publish`、`wasm.subscribe_namespace`
の span の中で答え、リクエストが TRACE_CONTEXT を持っていれば、その span を
`client_telemetry::continue_trace_from_traceparent` で相手の request span の子にする。

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
`MOQTClient` の callback export を残し、新しい `Session` は別の wasm クラスにした。
example を1つずつ別の PR で移し、最後の1つのあとで callback のクラス、`ClientState`、
`MoqtClientWrapper` を消した。最初に移したのは、core API の一番の利用者である
`LivePlayer` で、「L1 は alias ごとの handler だけを登録する」というルールもこれで
不要になった。

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
  できる。パッケージは Vite の app build（library mode は wasm と worker を data URL に
  埋め込む）で作り、出力の URL を `new URL('<相対パス>', import.meta.url)` の形に
  そろえる。
- **AudioWorklet**: Safari の音声 capture の module は、ソースを文字列として取り込み、
  Blob URL から `addModule` する。小さい asset は利用者の bundler が data URL に埋め込む
  ので、asset にはしない。
- **CSS**: JS は CSS を読み込まない。`LivePlayer` を出すページは
  `moq-wasm/media/player.css`、`PlayerControls` も出すページは
  `moq-wasm/media/controls.css` を読み込む。
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
   VP8/VP9/AV1 の映像、CMAF の opt-in**（PR 5本）。
8. **feat(wasm): `MediaBroadcast`**（`Mp4Broadcast` の一般化）と、
   **feat(browser): 既存の encoder を使う `publish(session, namespace, MediaStream)`。**
   media example の publisher をこれに移し、その経路から `serveSubscribes.ts`、
   `SubgroupWriter`、`fetchReplay` を消す。
9. **feat(browser): `publish()` に rendition、画面共有、codec の選択を足す。**
   そのあと meeting
   example を移し、`mediaPublisher.ts` のプロトコル部分と `meetingCatalog.ts` の
   catalog 組み立てを消す。
10. **feat(browser): Safari 対応。** 映像と音声の capture の代替経路。実機の Safari で
    配信と再生を確かめる。
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
4. **catalog: `publish()` で指定できる。** track ごとの label、role、rendition を
   渡せる。独自フィールドは、使う用途が出るまで見送る（変更7）。
5. **Audio Level: 含める。** `publish()` は音声の object ごとにレベルを計算し、LOC の
   Audio Level property として送る（変更5）。
6. **CMAF の兄弟 track: opt-in。** `publish()` は既定で CMAF を作らず、
   `{ cmaf: true }` で作る。他の publisher（live-ingest など）の既定は変えない
   （変更8）。
7. **パッケージ名: `moq-wasm`。** `package.json` の `name` と import パスに使う。
   `docs/architecture/moqt/sdk.md` は3言語の SDK の名前を `moqt` に揃えると決めたので、
   その移行計画の改名の手順で `moqt` に変える。それまでは `moq-wasm` のままである。
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

1. パッケージの `package.json` の version を上げ、`CHANGELOG.md` にその version の
   節を足す。
2. `sdk-v<version>` の tag を push する。crate とは別に版を刻むので、tag には `sdk-`
   を付ける。
3. CI がその tag で wasm と TS をビルドし、`npm pack` の tarball を作り、同じ名前の
   GitHub Release に添付する。changelog の該当部分を Release の本文にする。

3 は `.github/workflows/sdk-release.yml` が行う。tag がパッケージの version を指して
いなければ止まり、`CHANGELOG.md` にその version の節が無くても止まる。添付する前に、手順11の CI と同じ
`check:pack` を通す。

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
