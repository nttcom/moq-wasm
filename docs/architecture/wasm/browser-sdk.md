# ブラウザ SDK 設計

## ステータス
実装済み。1.0 の範囲は「決定事項」にまとめた。配布は GitHub Release に添付する tarball で
行い、npm への公開は含まない（「配布」を参照）。パッケージ構成、公開 API、ここに書いた
不変条件を変える変更で、このファイルも同時に更新する。

## 目標
Web 開発者が npm パッケージを1つ入れるだけで、MoQ のメディア PubSub を数十行で
書けるようにする。relay に接続し、カメラとマイクを配信し、他人が配信する namespace
を再生する。同じパッケージで、独自データ（チャット、テレメトリ、制御）を運ぶアプリ
向けに、型付きの track 単位 API も提供する。

`examples/browser` は SDK の上で動く。

| 項目 | API |
| --- | --- |
| 接続 | `MoqSession.connect(url, { token, versions })`。`versions` は提示する draft（`draft-14` / `draft-18`）を優先順に並べ、省略すると draft-18、draft-14 の順 |
| namespace の再生 | `moq-wasm/media` の `new LivePlayer({ container, callbacks })`、`start(session, namespace)` |
| メディアの配信 | `moq-wasm/media` の `createBroadcast(session, namespace)` と `addSource(name, input)`（「相手ごとに再生し、source を名前で扱う」） |
| track 単位の PubSub | `MoqSession` の `subscribe` / `fetch` / `publishTrack` が返す `MoqSubscription` / `MoqFetch` / `TrackWriter` の handle と async iteration |
| パッケージ | `sdk/js/js` の `moq-wasm`（npm workspace）。examples は `source` condition で TS ソースをそのまま読む。`npm run build` が `dist`（JS、worker、wasm）と `types`（`.d.ts`）を作り、CI が `npm pack` の tarball を新しい Vite プロジェクトでビルドして確かめる |

## 利用コード

接続:

```ts
import { MoqSession } from 'moq-wasm'

const session = await MoqSession.connect('https://relay.example.com:4433', {
  // 関数なら、接続、GOAWAY による移動、再接続のたびに呼ばれる
  token: () => fetchTokenFromMyBackend(),
})
session.closed.then((info) => console.log('session closed', info))
```

カメラとマイクの配信:

```ts
import { createBroadcast } from 'moq-wasm/media'

const broadcast = await createBroadcast(session, ['room-1', 'alice'])
const camera = await broadcast.addSource('camera', {
  label: 'Camera',
  video: true, // SDK が getUserMedia する
  audio: true,
  main: { height: 720, bitrate: 2_500_000 },
  renditions: [{ name: '360p', label: '360p', width: 640, height: 360, bitrate: 600_000 }],
})

await camera.setVideo(screenStream.getVideoTracks()[0])
await camera.setMuted('audio', true)
await broadcast.close()
```

namespace の再生:

```ts
import { LivePlayer, PlayerControls } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

const container = document.querySelector<HTMLElement>('#view')!
const player = new LivePlayer({
  container,
  callbacks: { onStateChange: () => {}, onLiveFrame: () => {}, onLog: () => {} },
})
// シークバー、再生/一時停止、LIVE、音量、画質メニュー
new PlayerControls(container, player, () => {})
await player.start(session, ['room-1', 'alice'])
```

コントロールをページで作る場合は、`PlayerControls` を使わずに
`callbacks.onStateChange` で `player.state` を描く。

メディアを使わない track 単位の PubSub:

```ts
const chat = await session.publishTrack(['room-1', 'alice'], 'chat')
await chat.writeGroup(new TextEncoder().encode('hello'))

const subscription = await session.subscribe(['room-1', 'alice'], 'chat')
for await (const object of subscription) {
  console.log(object.groupId, new TextDecoder().decode(object.objectPayload))
}

for await (const event of await session.watchNamespaces(['room-1'])) {
  if (event.kind === 'announced') void player.start(session, event.namespace)
}
```

## パッケージ構成

レイヤーごとにパッケージを分けず、subpath exports を持つ npm パッケージを1つにする。

```
moq-wasm            core: init, MoqSession, MoqSubscription, MoqGroup, MoqFetch, MoqNamespaceWatch, TrackWriter, RequestError, 型
moq-wasm/media      createBroadcast(), watchBroadcasts(), RemoteBroadcast, BroadcastCatalog, PhoneCall, LivePlayer, PlaybackGroup, PlayerControls, parseCatalog, Mp4Broadcast
moq-wasm/media/player.css
moq-wasm/media/controls.css
moq-wasm/recording  openRecording, Recording, RecorderError
moq-wasm/element    <moq-player>（import すると登録される）
```

1パッケージにする理由: `core` と `media` は1つの wasm インスタンスと1つの `Session`
を共有する。パッケージを2つに分けると、それぞれが wasm glue のコピーを持つ。両者の
バージョンがずれるとページに wasm インスタンスが2つでき、wasm の初期化が重なると
ヒープが壊れることがすでにわかっている。subpath exports なら tree shaking が効き、
視聴だけ、データだけのバンドルは小さく保てる。`media` はプレイヤー、decoder と
encoder の worker、CSS を引き込むが、`core` はどれも引き込まない。`recording` は
recorder の HTTP API だけを持ち、wasm も `media` も引き込まない。

ソースの置き場所: Rust 側は `sdk/js`、TypeScript 側は `sdk/js/js/`。ビルドは `wasm-pack build --target web`、`tsc`、
worker のバンドルで行う。`examples/browser` は workspace のパスでこのパッケージに依存する。

## 配信を `crates/broadcast` に寄せる

`crates/broadcast` は、すでに ingest-gateway（listen と pull）、gst-plugin-moqt、Live Viewer
の MP4 publisher（`sdk/js/src/mp4_broadcast.rs`）向けに MSF broadcast を組み
立てている。担っているのは、catalog、LOC（と CMAF）の track、media timeline、
catalog より先の PUBLISH、壁時計から始める group id、`moqt::Origin` による60秒の
FETCH キャッシュ、GOAWAY による切り替え、catalog の再送である。入力は、映像が
H.264（Annex-B）、H.265、VP8、VP9、AV1、音声が AAC、Opus、A-law、MP3 である。

```
JS（メインスレッド / worker）              wasm（sdk/js -> crates/broadcast）
-------------------------                 ---------------------------------------
MediaStreamTrack
  -> MediaStreamTrackProcessor
  -> VideoEncoder / AudioEncoder
  -> EncodedVideoChunk / EncodedAudioChunk --> MediaSource.pushVideo / pushAudio
     （Uint8Array に copyTo、timestamp µs）       -> BroadcastWriter::push(MediaEvent)
                                                  -> catalog、LOC、timeline、Origin、
                                                     PUBLISH、FETCH、SUBSCRIBE への応答
```

JS に残すのは、capture、encoder の設定、keyframe の要求、bitrate の制御で、どれも
ブラウザが必要なものである。プロトコルと MSF に関わる処理は、すべて wasm が行う。採番、キャッシュ、FETCH・
FETCH_CANCEL・TRACK_STATUS への応答は `Origin` が、SUBSCRIBE への応答と catalog の
組み立て（`broadcast::build_catalog`）は `broadcast` が担う。


下の層の変更は次のとおり。

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
   CMSF の switching set である。`ingest-gateway` は、transcode した rendition を
   `GroupAlignment` を使って `Broadcast::send_object` で書いている。ブラウザでは、
   rendition ごとに別の `VideoEncoder` へ同じ `VideoFrame` を渡し、同じフレームで
   全員に keyframe を要求するので、group id は完全に揃う。ingest-gateway の経路は
   `broadcast` の `RenditionTrack` に移し、`MediaBroadcast` はその上に
   `addRendition(name, label, bitrate)` を出す。encoder の出力は順不同で届くので、
   ブラウザの rendition は `RenditionKeyframes::SameFrames` で、source より先に
   来た keyframe でもその時刻の group id を割り当てる。
4. **track のライフサイクル。** ミュート、ソースの差し替え、画面共有では、track が
   catalog から抜けて戻る必要がある。そのため `Broadcast::remove_track`（catalog の更新と PUBLISH_DONE）がある。
5. **Audio Level。** meeting の話者検出は、LOC の Audio Level property
   （draft-ietf-moq-loc-04）を読む。JS が、今の `media/internal/audioLevel.ts` と同じく
   capture したサンプルからレベルを計算し、
   `pushAudio` に渡す。`broadcast` はそれをその object の property として書く。
6. **映像 codec の追加（H.265、VP8、VP9、AV1）。** `TrackPublisher` は
   `VideoConfig` の全 variant を受け取り、
   catalog の `codec` はビットストリームから読んだ config で作る。VP9 の level は
   ビットストリームに無いので、CMAF の vpcC box と同じく画面サイズに収まる最小の
   level にし、catalog と init segment の codec を揃える。CMAF の兄弟 track は既存の
   fMP4 muxer（`vp08`/`vp09`/`av01`）で作る。ブラウザからの AAC は、WebCodecs の
   `decoderConfig.description`（AudioSpecificConfig）から `AudioConfig::Aac` を作れば、
   既存の経路にそのまま乗る。プレイヤー側は catalog の codec 文字列で WebCodecs の
   decoder を設定するので、`VideoDecoder.isConfigSupported` で弾くだけでよい。
7. **catalog の独自フィールド（見送り）。** MSF は、仕様のフィールドと衝突しない独自
   フィールドを producer が足すことを認め（MAY）、parser には知らないフィールドを
   無視することを求めている（MUST、draft-ietf-moq-msf-02 §5）。parser 側の要件は
   msf の読み込みがすでに満たしている。producer 側は、仕様のフィールドで表せない
   情報（参加者の表示名など）を載せる用途が出るまで足さない。足すときは、MSF の
   フィールド名と衝突するキーをエラーにする（MUST NOT）。label と role は独自
   フィールドではなく、MSF の `label`、`role` に入れる。
8. **CMAF の opt-in。** `broadcast` は既定ですべての media track に `_cmaf` の兄弟
   track を作る。`SourceTracks` に CMAF を作るかどうかのフラグがあり、ブラウザの配信だけ既定で切る。`addSource` の
   `cmaf: true` は、映像と rendition にだけ兄弟 track を作る（Opus の音声には作れない）。
   兄弟 track は LOC の track と encoder を共有し、encoder はどちらかに購読者がいる間
   動く。

## 映像 encoder のウォームアップ

Chrome（macOS、Chrome 156 で確認）では、プロセスで最初の `VideoEncoder` が使える
ようになるまでに数秒かかる。その間、worker にフレームが届かなくなり、カメラも
撮らなくなる。準備ができると、止まる直前に撮ったフレームがまとめて届く。それを
そのまま encode すると、数秒前のフレームが live として届き、受信側はその到着の
揺れでジッターバッファーを膨らませる。

そこで、`createBroadcast` の映像 worker（`media/internal/encoders/videoEncoder.ts`）
は、encoder を作るたびに最初の1フレームを keyframe として encode し、`flush()` が
終わるまで出力を捨てる（`EncoderWarmUp`）。その間に読んだフレームは閉じて
`frameDropped` として数える。`flush()` を待つので、
`latencyMode: 'quality'` のように先読みする encoder でもウォームアップは終わる。
捨てた出力が持つ `decoderConfig` は、最初の live の出力に付けて渡す。

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
`MediaSource` は `broadcast::MediaSource`（Python SDK と共有。`BroadcastWriter` を1つ持つ）を包み、ingest-gateway pull がカメラの profile
ごとにそうしているのと同じく、`SourceTracks` で、source `<name>` の映像と音声を
`<name>`/`<name>_audio` と名付ける。どのソースの track も、broadcast の
1つの catalog に載る。`MediaSource` が公開するのは `configureVideo`・`pushVideo`、
`configureAudio`・`pushAudio`、`addRendition`・`configureRendition`・
`pushRenditionVideo`、`removeTrack`。JS の `createBroadcast()` は、`addSource` ごとに
`MediaSource` を1つ作る。

メディアを使わない配信（チャット、データ）には `broadcast` は要らない。
`moqt::Origin` を直接使う `session.publishTrack` で行う。broadcast の catalog に
`chat` を載せるには `createBroadcast(session, namespace, { offerChat: true })` を使い、
書き込みは同じく `session.publishTrack(namespace, 'chat')` で行う。

## 会議の再生を `LivePlayer` に載せる

### 方針
メンバーのソースごとに `LivePlayer` を1つ作り、会議が要る機能を `LivePlayer` に足す。
`LocLive` などの部品は公開しない。公開すると内部の API が公開範囲に入る。

1つの session に `LivePlayer` を複数載せる形は、remote-monitoring も使っている。
タイルは player に container の要素を渡し、React はその要素を置く場所を変えるだけにする。

1人のメンバーのカメラと画面共有は1つの broadcast に載るので、ソースごとの player が
それぞれ catalog を購読すると、1つの session で同じ track を2回購読することになる。
draft-ietf-moq-transport-14 §9.7 はこれを禁じている（subscriber は MUST NOT、publisher は
protocol violation として扱う SHOULD）。meeting もタイルの配置と画質の選択に catalog を読む。
そこで、broadcast の catalog を1回だけ購読する `BroadcastCatalog` を `moq-wasm/media` に置き、
`LivePlayer.start(session, namespace, catalog)` で player に渡す。player は catalog の
listener になり、自分では catalog を購読しない。ページも同じ `BroadcastCatalog` の
listener になる。publisher がいないときの再購読と、catalog の PUBLISH_DONE の扱いも
`BroadcastCatalog` が持つ。`start` に catalog を渡さない player は自分用の
`BroadcastCatalog` を作り、`stop` で閉じる。

### 足す機能

| 機能 | API | meeting での使い方 |
| --- | --- | --- |
| ソースの選択 | `LivePlayerOptions.source`（`{ video, audio? }`）。名前の映像 track と、その altGroup（draft-ietf-moq-msf-02 §5.2.13）の rendition、名前の音声 track だけを購読する。省略時は今と同じ | カメラと画面共有で player を分ける |
| 表示の高さで ABR の上限を決める | 自動選択のとき、container の高さを CSS ピクセルで監視し、ladder をその高さを満たす rendition までに絞る（今の `capLadderToHeight`）。devicePixelRatio は掛けない。高密度の画面では rendition をその倍率まで拡大して表示する | 小さいタイルは低い rendition を受ける |
| Audio Level | `LivePlayerCallbacks.onAudioLevel?(level: AudioLevel)`。音声 object の LOC Audio Level property（draft-ietf-moq-loc-04 §2.3.3.2）を渡す。`AudioLevel` を export する | 話者の検出（`dominantSpeaker.ts`） |
| 最初の映像を group の先頭から取る | 映像の最初の購読は `subscribeGroupStart` にする（下の「keyframe の間隔と NEW_GROUP_REQUEST」）。rendition の切り替えは新しい group を要求し、その先頭で切り替える | 静止した画面共有も、参加した直後に映る |
| 小さい track を映像より優先する | player が SUBSCRIBE する track の subscriber priority を、catalog 0、テキスト（media timeline、字幕）16、音声 32、映像 128 にする（`player/trackPriority.ts`、小さいほど先）。オプションにはしない。media timeline には、映像・音声と同じ `deliveryTimeoutMs` と `groupOrder` も付ける（毎回それまでの表を丸ごと送るので、遅れた object は届かなくてよい）。catalog には付けない | DELIVERY TIMEOUT のない track を映像と同じか下の優先度に置くと、下りが詰まったとき、映像の後ろで送られないままの stream が残る。relay が同時に開ける stream 数（subscriber が決める上限）をそれが埋め、優先度の高い映像の stream も開けなくなる |
| 映像どうしの優先度を変える | `LivePlayer.setVideoPriority(priority)`。映像の subscriber priority（0〜255、既定は128、小さいほど先）を変える。購読中の映像には SUBSCRIBE_UPDATE で送り、その後に購読する映像（rendition の切り替え、`setVideoEnabled(true)`）にも使う。音声は 32 のまま。view からは `view.player` で呼ぶ | 下りが詰まったとき、ピン留めした映像や画面共有をほかの映像より先に受ける |
| 古い映像と音声を送らせない | `LivePlayerOptions.deliveryTimeoutMs`（`RemoteBroadcastOptions` にも同名、既定は 2000、`null` で無効）。選択中の映像と音声、media timeline、`addAudioTrack` で足した音声の SUBSCRIBE に DELIVERY TIMEOUT（draft-14 §9.2.1.2）を付ける。catalog、字幕、`addTextTrack` には付けない。`null` と 1〜4294967295 の整数以外は `RangeError`。relay は object を受け取ってからその時間を過ぎた Group の stream を reset する | 下りが詰まったとき、遅れた映像を捨てて新しい映像を受ける。優先度だけでは、後回しの映像が送信バッファを埋めて優先した映像まで遅れる |
| 詰まったとき新しい Group を先に受ける | `LivePlayerOptions.groupOrder`（`RemoteBroadcastOptions` にも同名）。選択中の映像と音声の SUBSCRIBE の Group Order（draft-14 §7）。既定の `'ascending'` は古い Group から送るので、遅れが伸びる。`'descending'` は新しい Group を先に送るので、遅れずに先へ飛ぶ。SUBSCRIBE のあとでは変えられない | ライブ視聴で、遅れるより飛ぶほうがよい場合。`deliveryTimeoutMs` と併用すると、送れなかった古い Group は捨てられる |

#### keyframe の間隔と NEW_GROUP_REQUEST

配信側の keyframe は既定で 30 秒おきである（`encoding.keyframeIntervalSeconds`）。
encoder worker はフレームの timestamp で keyframe を選び、選んだフレームを全 rendition で
keyframe にするので、間隔はトラックの frame rate によらない。視聴側は次の keyframe を待たず、draft-18 の
NEW_GROUP_REQUEST（§10.2.13）で新しい group を要求する。

- **配信側。** `MediaBroadcast.addSource` は `onKeyframeRequest` を受け取り、映像と
  その rendition の track を DYNAMIC_GROUPS（§12.6）付きで配信する。要求が
  届くと wasm が callback を呼び、`sourceEncoding.ts` が encoder worker に
  `forceKeyframe` を送る。worker は次のフレームを全 rendition で keyframe にする
  ので、group id は rendition の間で揃ったままである。
- **誰も受け取らない track。** `MediaBroadcast.addSource` は `onDemandChange` も受け取り、
  映像・rendition・音声の track を Forward State 0 の PUBLISH で配信する（§10.10）。relay は
  Forward State 1 の購読者がいる間だけ Forward を 1 にし、wasm はその変化を track 名と
  一緒に callback へ渡す。`sourceEncoding.ts` は、映像と rendition なら encoder worker に
  `wanted` を送る。worker は `wanted` でない rendition にフレームを渡さず、再び `wanted` に
  なったら次のフレームを全 rendition で keyframe にする。音声は encoder を止め、再開の
  ときに新しい timeline で作り直す（音声の encoder は渡された sample 数で時刻を付けるので、
  入力を飛ばせない）。catalog と media timeline は Forward State 1 のまま配信する。
  track は最初の chunk で catalog に載るので、encoder は一度は動く。
  動きのない画面共有は track の開始時にしかフレームを出さないので、rendition が再び `wanted` に
  なったとき、page は track の clone を作って 1 フレームを読み、worker に `frame` として渡す
  （`refreshFrame`）。worker はそれを、すでに encode したフレームの後ろの時刻に置いて encode する。
  Safari（worker が track を読む経路）ではこの読み直しをしない。
- **最初の購読。** `subscribeGroupStart(namespace, track, 60)` を使う。SUBSCRIBE を送り、
  SUBSCRIBE_OK を受け取ってから次の要求を選ぶ。Largest Object が 60 object（30 fps で
  2 秒）より手前なら、joining FETCH を送って今の group を先頭から取る。それ以降なら
  NEW_GROUP_REQUEST を送り、新しい group から読む。1 秒待っても新しい group が来なければ、今の group を FETCH する。
  DYNAMIC_GROUPS の無い track と draft-14 の session は、いつも joining FETCH になる。
- **rendition の切り替えと `goLive`。** どちらも group の先頭からしか始められない
  ので、切り替え先を購読した直後と、Forward を 1 に戻した直後に
  `Subscription.requestNewGroup` を呼ぶ。
- **decode できなくなった group。** decoder がエラーで閉じたり keyframe を求めたりすると、
  video decoder worker は次の keyframe まで delta を捨てる。その待ちで最初に delta を
  捨てたとき、worker は `keyFrameNeeded` を一度だけ page に送り、`LivePlayer` は
  その group を指して `requestNewGroup` を呼ぶ。後の group がもう届いていれば、その
  keyframe を待つので要求しない。購読を始めた直後の待ちは最初の購読が group の先頭から
  取るので、要求しない。relay がサブグループのストリームを途中でリセットした group は、
  まだ要求しない。
- **relay。** 要求を上流へ送ったら、新しい group が始まるまで同じ track への要求は
  送らない（§10.2.13）。すでに始まった group への要求も送らない。送らなかったことは知らせない。
- **配信側の group の最短長。** `Origin` は、今の group が始まって 2 秒たつか 60 object に
  なるまで、要求を保留する（§10.2.13 が Original Publisher に認める遅延）。どちらかを
  満たした時点で keyframe を要求し、その前に次の group が始まれば要求を捨てる。最初の購読は
  60 object 以降でしか要求しないので、待たされない。待つのは切り替えと `goLive`、decode
  できなくなった group の要求で、長くても 2 秒である。
- **要求は 1 回だけ送る。** relay は、処理中の要求が無く、要求された group がまだ始まって
  いなければ、必ず上流へ送る。送り直しが relay を通るのは新しい group が始まった後だけで、
  そのときは group をもう 1 つ作らせるだけになる。

ソースは映像と音声の track 名で渡す。rendition は、catalog が switching set ごとに
付ける altGroup から見つかる。映像だけがミュートされて catalog から外れても、音声は名前で
見つかるので再生を続けられる。catalog の render group（§5.2.12）で映像と音声を結び付ける
方法は採らない。番号に意味が無いので、映像 track が catalog に無いときに、どの音声が
そのソースのものかを決められない。track 名（`addSource` の `<name>` と
`<name>_audio`）はアプリが渡し、player は命名の規則を持たない。
命名の規則は、player の上に置く `RemoteBroadcast` が持つ（「相手ごとに再生し、source を
名前で扱う」）。
`source` を `start` の引数でなく player のオプションにするのは、`start` の3つ目の引数が
すでに `LiveTracks` で、meeting はソースごとに player を作るため。

## 相手ごとに再生し、source を名前で扱う

### 利用コード

```ts
import { MoqSession } from 'moq-wasm'
import { createBroadcast, watchBroadcasts, type RemoteBroadcast } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'

const session = await MoqSession.connect(url, { token: () => fetchToken() })

joinButton.onclick = async () => {
  // 呼んだ時点で namespace を公開する
  const local = await createBroadcast(session, ['room', 'alice'], { onError: console.error })
  // SDK が getUserMedia する。加工した track もそのまま渡せる
  const camera = await local.addSource('camera', { label: 'Camera', video: true, audio: true })
  preview.srcObject = camera.stream

  const remotes = await watchBroadcasts(session, ['room'], {
    filter: (namespace) => namespace.at(-1) !== 'transcriber',
    onAdd: (remote) => addTile(remote),
    onRemove: (remote) => removeTile(remote)
  })
  leaveButton.onclick = async () => {
    await remotes.close()
    await local.close()
    session.close()
  }
}

function addTile(remote: RemoteBroadcast) {
  const tile = newTile(remote.namespace) // ページが作るタイル。ページが消す
  remote.listen({
    // 登録した時点で、今ある source が流し直される
    onSourceAdd: (view) => {
      tile.append(view.element) // SDK が作り、SDK が消す
      view.listen({ onChange: () => tile.render(view.name, view.state) })
    },
    onSourceRemove: (view) => tile.forget(view.name),
    onAudioLevel: (view, level) => tile.speaking(level)
  })
}
```

### source の名前
source 名は、その source の映像 track の名前である。名前の無い source は作らない。

| track | 名前 |
| --- | --- |
| 映像 | `<name>` |
| 音声 | `<name>_audio` |
| rendition | `<name>_<rendition>`（映像と同じ altGroup） |
| CMAF の兄弟、media timeline | `broadcast` の命名（`_cmaf`、`_timeline`） |

- source 名は識別子で、表示名は MSF の `label`（draft-ietf-moq-msf-02 §5.2.11）に入れる。
  独自フィールドは足さない（変更7の見送りのまま）。
- `addSource` は、空の名前、`catalog`、`chat`、`_audio`・`_cmaf`・`_timeline` で終わる
  名前、ほかの source の track と重なる名前を reject する。
- `crates/broadcast` を使う配信元（ingest-gateway、gst-plugin-moqt）は
  `video`/`audio` のままにする。受ける側には source `video` として見える。

`RemoteBroadcast` は catalog から次の規則で source を読む。

1. CMAF の兄弟と timeline は source を決めるのに使わない。player が source の
   track から見つける。
2. altGroup（§5.2.13）を共有する LOC track は1つの source にまとめる。その名前は、group の
   中で一番短い名前の映像 track（映像が無ければ音声 track）に 3 を当てはめて決める。
   rendition と、live-translate の言語ごとの音声はこれでまとまる。
3. 残りの映像 track は、その名前の source になる。ただし `<name>_<rendition>` の rendition が
   映像本体より先に載った（ミュートを解除したカメラ）ときは、音声が名付ける source `<name>`
   に入る。音声 track は、`<name>_audio` なら
   source `<name>`、`audio` なら source `video`（`broadcast` の既定の命名）、
   それ以外は自分の名前の、音声だけの source になる。
4. source は、その track が catalog に1本でもある間は存在する。カメラを止めても音声が
   残る間は `onSourceRemove` を呼ばず、view の `state.video` が `off` になるだけである。

### 送る側: `createBroadcast` と `LocalSource`

| API | 振る舞い |
| --- | --- |
| `createBroadcast(session, namespace, { offerChat?, eventTimelines?, onError? })` | 呼んだ時点で namespace を公開する。`offerChat` と `eventTimelines` は、`session.publishTrack` で書く track を catalog に載せる。`onError` は encoder のエラーを受ける |
| `local.addSource(name, input)` | source を足し、その track を catalog に載せる。デバイスの権限を拒否されたら `getUserMedia` のエラー（`NotAllowedError`）で reject する |
| `local.source(name)` / `local.removeSource(name)` / `local.close()` | `close` は全 source の PUBLISH_DONE を送り、session は閉じない |
| `source.setVideo(input \| false)` / `source.setAudio(input \| false)` | デバイスの切り替え。`false` はデバイスを離す |
| `source.setMuted(kind, muted)` / `source.isMuted(kind)` | デバイスを掴んだまま、送るのを止める |
| `source.stream` | 送っている track の `MediaStream`。自分のプレビューに使う。track を替えても同じ stream の中で差し替える |
| `source.stats()` | `SourcePublicationStats`（映像と rendition ごとの kbps・fps・frameSize、落としたフレームと encoder のエラーの数）。音声は encoder に入った `AudioData` の直近1秒の数、最新のサンプルレートと ch 数、RFC 6464 の音量（dBov）の最小とパワー平均も持つ。encoder worker が `AudioData` ごとに `{ type: 'input' }` を postMessage して数える |
| （ログ） | source の音声 track（`setAudio` で替えたものを含む）の `mute` / `unmute` / `ended` を、その時の `readyState` とともに INFO で書く（`audio track muted` など）。track を外すと書かなくなる |
| `source.listen({ onTrackEnded, onEnded })` | `onTrackEnded(kind)` は SDK が取ったその kind の track が終わった（デバイスが抜かれた）とき、`onEnded` は source の track がすべて終わった（ブラウザの「共有を停止」）とき。SDK は track や source を外してから呼ぶ。ページが渡した track の終了はページが扱う（取り直して `setAudio` で差し替えるなど） |

`input` は `{ label?, video?, audio?, stream?, display?, encoding?, main?, renditions?, cmaf? }`
である。映像の encode は、どの track に効くかで3つに分ける。

| フィールド | 効く track | 中身 |
| --- | --- | --- |
| `encoding` | 映像 `<source>` と全 rendition | `codec`、`keyframeIntervalSeconds`、`hardwareAcceleration`、`latencyMode`（既定は `'realtime'`） |
| `main` | 映像 `<source>` | `width`、`height`（大きさの上限）、`bitrate`、`framerate`（fps の上限。rendition も引き継ぐ） |
| `renditions` | それぞれ `<source>_<name>` | `name`、`label`、`width` と `height` か `scaleResolutionDownBy`、`bitrate`、`framerate`、`codec`（既定は `encoding.codec`） |

`label` は source の label で、映像の label と音声の label（後ろに ` audio`）になるので、
`main` ではなく `input` の直下に置く。`scalabilityMode: 'L1T1'` は SDK が固定し、利用者には見せない。

rendition の大きさは、`width`/`height`（上限）か `scaleResolutionDownBy`（取り込んだフレームの
縦横を割る。WebRTC と同じ名前と意味）のどちらかで指定する。倍率で指定すると、配信者は
カメラや画面の元の大きさを知らなくてよい。フレームの大きさが変わると encoder を設定し直す
仕組み（`configureForFrame`）があるので、画面共有のウィンドウの大きさが変わっても
倍率から計算し直すだけで済む。bitrate は大きさに比例しないので、倍率で指定しても必須にする。

WebCodecs の `framerate` は rate control の目安で、encoder はフレームを間引かない。
そこで `framerate` を指定したトラックは、encoder worker が timestamp を見て上限を超える
フレームを飛ばす（`frameRateLimit.ts`）。keyframe のフレームは飛ばさないので、どの
rendition も同じフレームで group が始まる。指定しなければ、届いたフレームをすべて
encode する。画面共有のように変化があるときだけフレームが届く入力は、上限より遅いので
何も飛ばさない。catalog の `framerate`（MSF §5.2.21、任意）は、上限を指定したトラックにだけその値を書く。
指定がないトラックの実際の fps は入力次第で分からないので、項目を省く。`source.setVideoEncoding({ encoding, main })` は2つを
置き換えて encoder を作り直す。

`video` と `audio` は次のどれかである。

- `true` か `MediaTrackConstraints`：SDK が `getUserMedia` で取る。両方を指定したら1回の
  呼び出しにまとめ、許可のダイアログを1回にする。
- `MediaStreamTrack`：ページが取った track（ノイズキャンセリングや背景ぼかしをかけたもの）。
  `stream` は、その最初の映像と音声の track を渡す糖衣。
- `display`：SDK が `getDisplayMedia` で取る。user activation の中で呼ぶ。

track の持ち主: SDK が取った track は、SDK が `setVideo`、`removeSource`、`close` で止める。
ページが渡した track は止めない。

ミュートと OFF は、相手からは同じに見える。どちらも track が catalog から抜け、view の
`state` が `off` になる。違いは自分の側だけにある（OFF はカメラのランプも消える）。

### 受ける側: `watchBroadcasts` と `RemoteBroadcast`

`watchBroadcasts(session, prefix, { filter?, onAdd, onRemove })` は `session.watchNamespaces`
の上で、公開された namespace ごとに `RemoteBroadcast` を作る。

- ハンドラは呼び出しの引数で受ける。監視の開始と処理の開始を一致させ、作ってから代入する
  間に届いた配信を取りこぼさない。relay が SUBSCRIBE_NAMESPACE を受け付けたら resolve し、拒否
  したらその `RequestError` で reject する。返り値は `close()` だけを持ち、`close()` は全部の
  `RemoteBroadcast` を片付ける（`onRemove` は呼ばない）。
- 同じ session が `createBroadcast` した namespace は除く。別の session（別のタブ）が同じ
  namespace で配信していれば除かない。
- `filter` で、文字起こしボットのような参加者以外の配信を除く。深さではなく関数にするのは、
  参加者と同じ深さにいる配信も除けるようにするため。
- namespace が withdraw されたら、SDK が view と catalog を片付けてから `onRemove` を呼ぶ。
  公開中の namespace がもう一度公開されたら（入り直し）、古いものを `onRemove`、新しいものを
  `onAdd` で知らせる。

`RemoteBroadcast` は1つの `BroadcastCatalog` を持ち、source ごとに view を作る。1人を
見るだけのページは `new RemoteBroadcast(session, namespace)` を直接使う。

| API | 振る舞い |
| --- | --- |
| `remote.listen({ onSourceAdd?, onSourceRemove?, onAudioLevel? })` | 登録した時点の source を `onSourceAdd` で流し直す（`BroadcastCatalog.listen` と同じ）。解除する関数を返す |
| `remote.sources` / `remote.catalog` | source 名から view への `ReadonlyMap` と、broadcast の `BroadcastCatalog` |
| `remote.dispose()` | view と catalog を片付ける |
| `view.name` / `view.label` | source 名と catalog の `label` |
| `view.element` | SDK が作る描画要素。source が消えたら SDK が取り除く |
| `view.state` / `view.listen({ onChange?, onPlayerChange? })` | `{ video: 'waiting' \| 'playing' \| 'off', audio: 'playing' \| 'off', videoSize? }` |
| `view.setVolume(volume)` / `view.stats()` | `LivePlayer` の `setVolume`、`stats` |
| `view.setVideoEnabled(enabled)` | 映像の購読を止めて音声を残す |
| `view.player` | seek や track の選択など、ほかの `LivePlayer` の操作 |

`RemoteBroadcast` の option のうち `audioContext` と `keepLiveWhileReviewing` は、各 view の
`LivePlayer` にそのまま渡る。

`view.element` の決まり:

- 要素はページが与えた箱いっぱいに広がり、映像の収め方は CSS 変数 `--moq-object-fit`
  で決める（既定は `contain`）。大きさはページの CSS が決める。
- `data-source` に source 名を、`data-video` と `data-audio` に `state` の値を付ける。
  アバターやマイクの印を、ページが CSS だけで描ける。
- SDK は要素を IntersectionObserver で見て、画面外の間は映像の購読を止める。
  `RemoteBroadcast` の option で切れる。ページが `setVideoEnabled(false)` にした view は
  自動では戻さない。
- ページは要素を別の場所に移してよい。同じタスクの中で移せば映像は止まらない。HTML の
  media 要素が document から外れたときの一時停止は、安定状態になってもまだ外れている
  ときにだけ起きる。canvas はもともと影響を受けない。テストで守る。

### 音声の出力
ページが1つの `AudioContext` を持ち、`RemoteBroadcast`（`watchBroadcasts` の option も
同じ）の `audioContext` で渡す。各 view の `LivePlayer` は、その context で live の LOC の音を
鳴らす（`LivePlayerOptions.audioContext`）。ページは user gesture の中でその context を
resume し、スピーカーは `audioContext.setSinkId(deviceId)` で選ぶ。`watchBroadcasts` の
`onAdd` は click の外で起きるが、ページが click の中で resume した context で鳴るので、後から
入った人の音も聞こえる。SDK が context を作って持つ API（`unlockAudio`、`setAudioOutput`）
は足さない。context をページが持てば、ページは resume、出力先、close の時機を自分で決められ、
SDK は context を1つも持たずに済む。

### session の後始末
`pagehide` で session を閉じる option は足さない。Chrome はページを閉じるときもリロード
するときも WebTransport を自分で閉じ、relay はその session の namespace をすぐ withdraw する
（`remote-broadcast-e2e.spec.ts` で publisher のページを閉じて確かめた）。古い session が
relay に残るのは、ブラウザが接続を閉じられないとき（クラッシュ、通信断）で、そのときは
`pagehide` で閉じることもできない。

### meeting での使い方
meeting は `createBroadcast` で配信する（`mediaPublisher.ts`）。受ける側は
`watchBroadcasts` ではなく、`session.watchNamespaces` と `new RemoteBroadcast` を直接
使う（`localSession.ts`、`remotePlayers.ts`）。

入り直し（リロードした人の古い session が残る間の振る舞い）は、現象を再現して原因を
確かめてから決める。relay は、同じ namespace を公開する session が残っている間は
PUBLISH_NAMESPACE_DONE を流さない。namespace に SDK が一意な要素を足す方法は採らない。
名前で配信を指す使い方（`['live', 'concert']`）と、namespace で配信を探す recorder や
live-translate が影響を受けるためである。

## 電話をつなぐ `PhoneCall`
SIP gateway（docs/architecture/sip-gateway/architecture.md）がつないだ電話と、ページを
1対1で話させる。ページは電話をかけない。アプリのサーバーが gateway にかけさせた通話か、
受けさせた通話に、`call_id` と namespace を受け取ってから入る。

```ts
const call = await PhoneCall.join(session, {
  call: ['app', 'room', 'phone', callId], // gateway の配信
  local: ['app', 'room', 'alice', 'call', callId], // アプリのサーバーが gateway に渡した peer
  audio: true,
  audioContext
})
call.listen((state, reason) => {}) // dialing | ringing | answered | ended
await call.sendDtmf('1#')
await call.hangUp()
```

- **送る側:** `createBroadcast(local)` にマイクだけの source `microphone` を足し、同じ
  namespace に `control` track を `session.publishTrack` で出す。`control` には命令を
  1つずつ、それだけの Group にした JSON で書く（`{"event":"dtmf","digit":"1"}`、
  `{"event":"hangup"}`）。
- **受ける側:** 電話の音声は `RemoteBroadcast(call)` で鳴らす。状態は gateway の `call`
  track を joining FETCH つきで購読して読む（`CallStates`）。gateway がまだ公開して
  いなければ 500 ms ごとに購読し直す。
- **切る:** `hangUp` は `control` に `hangup` を書いてから、マイクと再生を止める。
  `LocalBroadcast` は session を閉じるまで namespace を公開し続け、ページの session には
  ほかの配信も載っているので、配信を終えて切ることはできない。session が閉じたとき
  （ページを閉じた、落ちた）は、gateway が PUBLISH_NAMESPACE_DONE を見て切る。
- **終わり:** `ended` を読むか、`ended` を読まないまま `call` track が終わる（gateway が
  止まった。`reason` は `gateway`）と、マイクと再生を止める。

## 録画を `LivePlayer` で再生する

### 方針
MoQ では、過去の Object も FETCH で取れるので、録画とライブの違いは「publisher がまだ
配信しているか」だけになる。録画を見るときは、元の publisher の代わりに recorder が
保存した Object を配信する（replay）。そこで、録画を **MSF の VOD broadcast** として
replay し、`LivePlayer` は catalog が示すとおりに再生する。

- replay の catalog は、各 track を `isLive: false`（draft-ietf-moq-msf-02 §5.2.8）にし、
  `trackDuration`（§5.2.36）を付け、catalog 全体を `isComplete: true`（§5.1.3）にする。
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
- recorder と player は、ソースを `renderGroup`（§5.2.12）で、rendition を `altGroup`（§5.2.13）で
  見分ける。カメラか画面共有かを示す独自のフィールドは足さず、recorder は既定のソースの track 名
  （`video`、`audio`、`camera`）でカメラを見分ける。表示には `label` を使う。
- recorder の HTTP API（再 publish の依頼と lease）は `LivePlayer` に入れない。SDK の
  別の subpath（`moq-wasm/recording`）に置き、`LivePlayer` とライブだけ使うアプリの
  バンドルには入れない。

### publisher（`crates/broadcast`）

| 項目 | 振る舞い |
| --- | --- |
| timeline を出すソース | すべてのソース。ソースごとに timeline track を1本出し、そのソースの映像、rendition、音声を対象にする。`depends` は、それらがソースの renderGroup のすべてならその番号（§7.2）、そうでなければ track 名を並べる。track 名は `<ソースの映像の track 名>_timeline`（既定のソースは `video_timeline`） |
| entry を書く時 | ソースの group が始まるごと。映像があるときはキーフレームで、無いとき（ミュート中、音声だけのソース）は音声が始める group で書く。1つのソースの映像と音声は同じ Group ID の列を使うので、1本で両方の位置を表せる |
| group ID が飛んだとき | 音声の group でも entry を書くので、ID は飛ばない。一覧を空にするのは publisher が配信をやり直したときだけにする |

保持する範囲は relay のキャッシュの保持時間（60 秒）とする。

### recorder（replay）

| 項目 | 振る舞い |
| --- | --- |
| track の選び方 | catalog の `renderGroup` ごとに1ソースとして録画し、`altGroup` の中から `--max-video-height` 以下で一番高い1本を選ぶ。上限はすべてのソースで同じで、既定を 720 にする。チャットは `eventtimeline` の track を catalog から選ぶ |
| catalog | replay の namespace ごとに、保存した track を並べた catalog を1つ作って流す。各 track は元の catalog から codec、幅、高さなどを引き継ぎ、`isLive: false` と `trackDuration` を付ける。catalog 全体は `isComplete: true` にし、`generatedAt` は付けない（§5.1.2） |
| 時刻から位置 | 元の publisher の timeline は録画しない。replay は、保存した group ごとの最初の Object の Timestamp（録画の開始からの media time、下の段落）から、ソースごとに全期間の media timeline を1つ出す。presentation time はその Timestamp、wallclock は 0（draft-ietf-moq-msf-02 §7.1.1） |
| joining FETCH | 受ける。catalog と timeline は1 group なので、joining FETCH で全体が届く |
| 終わり | catalog の `isLive: false` で示す。SUBSCRIBE は Object を送らずに続け、lease が切れたら session を閉じる。PUBLISH_DONE は送らない |
| 参加者の名前 | 元の namespace の、room より後の要素（会議では参加者名）を `<member-index>` の代わりに入れる。最後の要素だけにしないのは、room より深い namespace どうしで名前が重ならないようにするため |
| transcript | transcriber の記録を話者ごとに分け、話者のカメラの replay namespace に `transcript` として載せる。transcriber の namespace は announce しない。replay の記録は media time なので、§8.3 は `depends` にその media timeline を並べることを求め（MUST）、§5.2.15 はその名前を同じ namespace で引く。参加者の media timeline はその参加者の namespace にあるので、そこに置く |
| HTTP の応答 | replay の prefix（`namespace_prefix`）、`renew_after_seconds`、録画の開始時刻（`started_at_ms`）だけにする |

transcript の記録は、話者（room の直下の要素が話者名の参加者）のカメラの session のうち、録画した
メディアの範囲が記録の `M` を含む最初のものに入れる。当てはまる参加者や session が無い記録と、音声を
録画していない session の記録は捨てる。catalog では `packaging` を `eventtimeline`、role を
`subtitle`（Table 4）、`eventType` を `io.skyway.research.moqt.transcript.v1`、`depends` をその namespace の
media timeline にし、`trackDuration` はほかの track と同じにする。track は group 0 の Object 1つで、その
session の記録を `M` の順に並べ、同じ記録は1つにまとめる。

`trackDuration` は replay の namespace の全 track で同じ値にし、最初の media の Timestamp から
最後の media の Timestamp までのミリ秒とする。

録画の時刻は、録画の開始を起点にした1つの時計に揃える。recorder は保存するときに、LOC の
Timestamp を開始からの media time（Timescale 1,000,000、draft-ietf-moq-loc-04 §2.3.1.2）にし、
chat などの event timeline の wallclock の記録（`T`）を media time の記録（`M`、
draft-ietf-moq-msf-02 §8.2）にする。開始より前のものは保存しない。こうすると参加者全員が1つの
時間軸に乗り、timeline の wallclock は §7.1.1 が VOD に勧めるとおり 0 にできる。replay は保存した
Object をそのまま出す。会議の時刻は、publish の応答の `started_at_ms` に media time を足して求める。

replay が作る catalog と timeline は、保存したデータだけから決める。draft-ietf-moq-transport-14
§2.1 は、Object がどこから取っても同じバイト列で、中身が時間で変わらないことを求める
（MUST NOT change）。同じ録画の replay は Cloud Run の別のインスタンスでも立ち、同じ
namespace を PUBLISH_NAMESPACE する。relay はその全 publisher から取り込み、重複を1つに
まとめる（§8.2、relay の architecture.md の Ingress）。作った時刻やインスタンスごとの値を
入れると、同じ位置に別の中身が並ぶ。

同じ録画の replay が複数のインスタンスで立つことは許す。recorder を1インスタンスに
しても、デプロイでリビジョンが切り替わる間は新旧が並ぶので、どのみち重複に耐える必要が
ある。上の決まりを守れば、重複は relay がまとめる。

### SDK

`LivePlayer`:

- catalog の track が `isLive: false` のとき、シークバーの範囲を timeline の最初から最後
  （`trackDuration`）にし、`goLive` を出さない。再生位置が終わりに着いたら止まり、
  `state.ended` を立てる。
  - 再生位置は LOC の Timestamp で表す。timeline の wallclock が 0 の記録は、presentation time の
    位置に置く（replay は group の最初の Object の Timestamp を presentation time にしている）。
  - 映像と音声は SUBSCRIBE しない（Object が届かない）。全部を巻き戻しと同じ FETCH の窓で読む。
    timeline も Object が1つだけなので、catalog と同じく joining FETCH で読む。
  - timeline が届いたら最初の group から再生する。ライブの視聴と同じく、先に一時停止して
    いなければそのまま再生する。終わりでは一時停止し、そこから再開すると最初から再生する
    （メディア要素と同じ）。
- 見たことのない group へも、timeline の entry を使ってシークする。FETCH の窓は巻き戻しと
  同じ（group の先頭から、4 group ずつ）。ライブでも、timeline があればこれを使い、relay の
  キャッシュに無い範囲は `FetchFailure` で扱う。
- 複数の player を1つの再生位置に揃える `PlaybackGroup` がある。会議で全員を 10 秒前に
  戻してまとめて見る、録画の会議を全員そろえて再生する、といった使い方のため。映像と音声が
  1つずつの配信は、1つの player の中で揃うので使わない。
  - group は1つの AudioContext と、LOC の Timestamp（ライブでは capture 時刻、録画では録画の
    開始からの media time）で表した1つの再生位置を持つ。`play`、`pause`、`seek`、`skip`、
    `goLive` を全 player に当てる。
    揃えている間の音声はこの AudioContext で鳴らし、ライブの間はそれぞれの player の
    ライブの再生経路で鳴らす。
  - ライブの間、player はそれぞれのライブの先端で再生する。揃えるのは巻き戻している間と、
    録画（`isLive: false`）を再生している間だけにする。
  - `group.add(player)` と `group.remove(player)` で途中から出入りできる。group の player は
    group から操作し、1人だけ巻き戻すときは group から外す。戻すと group の再生位置に揃う。
    group にいる player の `seek`、`skip`、`goLive`、`setPaused` は group に当たる。
  - 再生位置が player の範囲の外にある player は、何も FETCH せず、状態で範囲外を示す。
    範囲は、録画では timeline の最初の entry から `trackDuration` の長さ、ライブでは timeline
    の最初の entry からライブの先端まで。timeline にあっても relay のキャッシュから消えた
    範囲は、上の `FetchFailure` で扱う。
    途中で入った参加者、先に抜けた参加者、入り直して namespace が分かれた参加者は、
    これで同じに扱える。

`moq-wasm/recording`:

- `openRecording(session, recorderUrl, id, { token })` が、`POST /recordings/{id}/publish`
  の応答を開いたまま持ち、replay の prefix と録画の開始時刻（`startedAtMs`）を返す。
  `close()` で応答を閉じる。`session` が
  閉じたときも閉じる。`token` は `connect` と同じく文字列か非同期関数で、関数なら応答を
  開くたびに呼ぶ。recorder が断ったら、HTTP の status を持つ `RecorderError` で reject する。
- `renew_after_seconds` ごとに次の応答を開き、60 秒重ねてから前の応答を閉じる。新しい
  応答を開いた後の FETCH は、relay が新しい publisher に送る。古い replay に向かった FETCH
  は、relay が FETCH_OK を 10 秒（`moqt` の `CONTROL_MESSAGE_RESPONSE_TIMEOUT`）、データを
  20 秒（`MOQT_FETCH_FILL_TIMEOUT_SECS` の既定）待って打ち切るので、合わせて 30 秒より
  長く重ねれば、replay が閉じて切れることはない。`Session` にも player にも手を入れない。
- ページが閉じていない応答が終わったら、すぐ次の応答を開く。開けなければ、`closed` が
  そのエラーで解決する。`renew_after_seconds` の時に開けなかったときは、今の応答を持ち続け、
  それが終わったときにもう一度開く。

会議の録画を再生するアプリは、`openRecording` で prefix を得て、`watchNamespaces(prefix)`
で参加者ごとの namespace を知り、ソースごとに `LivePlayer` を作って1つの `PlaybackGroup`
に入れる。namespace の参加者名で player をタイルにまとめ、その時刻に範囲内の player を
映す。チャットと字幕は、catalog の event timeline track をアプリが FETCH して並べる。
`examples/browser/examples/recording-player` がこの形で再生する。event timeline は
`eventType`（§5.2.6）で見分け、`MediaCatalogTrack` がそれを持つ。track の終わりは
SUBSCRIBE_OK の Largest Location で知り、そこまでを1つの FETCH で読む。relay は購読して
いない track の TRACK_STATUS に答えないので、TRACK_STATUS は使えない。player が catalog を
購読し続けるので、ページは player を始める前に catalog を読む。

### 範囲外
- MP4 で保存した録画の replay。MP4 には元の catalog も timeline も残らないので、VOD の
  broadcast にするには、サンプルの時刻から catalog と timeline を作る経路を MoQ 形式とは別に
  持つことになる。MP4 の録画ファイルと合成は残る。
- 録画の時刻を開始からの media time にする前の録画（room manifest の `version` が 1 のもの）。
  後方互換は持たず、replay しない。
- カメラか画面共有かを catalog で示すこと。source 名（「相手ごとに再生し、source を名前で
  扱う」）と `label` で足りる。画面共有だけ高い解像度で録画するなど、種類で振る舞いを変える
  必要が出たときに決める。

## `<moq-player>`

`LivePlayer` を HTML のタグ1つで置けるようにする Custom Element。`moq-wasm/element` を
import すると `customElements.define('moq-player', …)` が走る。副作用を持つのはこの entry
だけで、`moq-wasm/media` は import しても何も登録しない。

```html
<moq-player src="https://relay.example/moq" namespace="room/alice" token="…" controls></moq-player>
```

- **属性。** `src`（relay の URL）、`namespace`（`/` 区切りの track namespace）、`token`、
  `controls`、`packaging`。`namespace` と、`src` か `session` プロパティがそろい、ページに
  入っている間だけ再生する。`src`、`namespace`、トークン、`session`、`tracks` が変わると
  player を作り直し、ページから外れると player を `dispose` して、`src` から開いた session
  を閉じる。開始中の変更はまとめ、開始が終わった時点の値で1回だけ作り直す。
- **ページの session。** `session` プロパティに入れた session で再生し、`src` には接続
  しない。同じ session で別の track も扱うページのためで、要素はこの session を閉じない。
- **`tracks` と `packaging`。** `tracks` は `LivePlayer.start` に渡すトラック。`packaging`
  属性は `setPackaging` に渡し、再生中に変えても作り直さない。
- **トークン。** `token` 属性は文字列。`token` プロパティには `MoqConnectOptions.token` と
  同じく、接続のたびに呼ばれる関数も入れられる。属性に書いたトークンは HTML から読めるので、
  期限の短いトークンを使うページはプロパティに関数を入れる。
- **`controls`。** `PlayerControls` を出す。外すと `PlayerControls.dispose` で取り除き、
  接続はそのまま続く。
- **見た目。** Shadow DOM は使わず、要素自身を `LivePlayer` の container にする。ページは
  `LivePlayer` と同じく `player.css`（`controls` なら `controls.css` も）を import する。
- **それ以上の操作。** `player` プロパティが再生中の `LivePlayer` を返す（`setVolume`、
  `stats` など）。`LivePlayerCallbacks` の `onStateChange` と `onLiveFrame` は、要素の
  `statechange` と `liveframe` イベントになる。接続や再生の開始に失敗すると、要素に
  `error` の `ErrorEvent` を投げる。`LivePlayerOptions`（`deliveryObserver` など）は渡せ
  ない。それが要るページは `LivePlayer` を直接使う。

## core API

core API は、制御メッセージをそのまま出すのではなく、Rust の crate にすでにある
handle 型（`Subscriber`、`Publisher`、`Subscription`、`TrackReader`、`TrackWriter`、
`Origin`）で `moqt::Session` を包む。ルールは次のとおり。

- 公開 API に request id、track alias、subgroup id を出さない。request id は moqt
  が振り、Maximum Request ID も moqt が守る。handle は、自分がどのリクエストから
  生まれたかを知っている。
- 1リクエストに1 handle。`subscribe` は SUBSCRIBE_OK で `MoqSubscription` に解決し、
  失敗すると draft のエラーコードを持つ `RequestError` で reject する。handle の
  メソッドは SUBSCRIBE_UPDATE と UNSUBSCRIBE（`close()`）である。
- データは session 全体の callback ではなく、async iteration で届く。
  `MoqSubscription` は、subgroup stream ごとに group 順・object 順で並ぶ
  `AsyncIterable<ReceivedObject>` である。stream を分けて扱いたい利用者（プレイヤーは
  映像を group ごとに decode する）のために、`groups(): AsyncIterable<MoqGroup>` も
  用意し、各 `MoqGroup` は `AsyncIterable<ReceivedObject>` とする。背圧は iterator が担う。
  JS が次を求めたときに、wasm が次の object を読む。
- session 単位のイベントは、promise（`session.closed`）と async iterable
  （`session.watchNamespaces(prefix)`）で出す。ページ全体で1つしかない callback の
  枠は作らない。
- session の状態は `session.isOpen` と `session.closed` で出す。再接続中も handle は
  生きたままで、その間の状態は出さない（「再接続」を参照）。
- 境界をまたぐ値は `Uint8Array` と `bigint`。`ReceivedObject` は
  `{ groupId, subgroupId, objectId, objectStatus, objectPayload, objectPayloadLength, locHeader }`。
- wasm 内の `tracing` のログは `init({ onLog, logLevel })` で受け取る。`onLog` は
  `logLevel`（既定は `'warn'`）以上の event ごとに `LogRecord`
  （`{ timeMs, level, target, message, fields, traceId?, spanId? }`）で呼ばれる。`onLog` を渡さなければ
  ログは捨てる。`onSpan` も渡していれば、span の中で書かれたログには
  その span の `traceId` と `spanId` が付く。wasm の subscriber はページ全体で1つなので、session の option では
  なく `init` の option にし、最初の `init` の option だけが効く（後の呼び出しは最初の Promise を返す）。
  subscriber は `tracing-subscriber` の registry に自前の `Layer` を載せたもので、
  `sdk/js/src/log_sink.rs` にある（`tracing-subscriber.md` を参照）。
- `init({ onSpan })` を渡すと、閉じた INFO 以上の span（request span
  `moqt.subscriber.subscribe` など）が `SpanRecord`
  （`{ traceId, spanId, parentSpanId?, name, startTimeMs, endTimeMs, fields }`、id は小文字 hex）で
  渡る。`logLevel` は span には効かない。span を記録する `Layer` は
  `crates/telemetry-report/src/span_record.rs` にある。
- `onLog` と `onSpan` は、レコードが属する session のトークンを第2引数で受け取る
  （provider なら最後に返した値）。session に属さないレコードと、トークンのない
  session のレコードでは `undefined`。テレメトリのレポートの `token` に入れるためのもので、
  レコードの `fields` には入れない（ClickHouse にそのまま残る）。アプリは何もしなくてよい。
  - 仕組み: `MoqSession.connect` ごとに wasm が `telemetry-report` の scope（番号）を1つ
    割り当て、トークンを scope に結び付ける。session と、それが返すオブジェクト
    （`Subscription`、`MediaBroadcast` など）のメソッドは、その scope の中で poll される。
    moqt の browser 用 `executor::spawn` は `set_spawn_wrapper` で、spawn した側の scope を
    タスクに引き継ぐので、session のタスクと broadcast のタスクも同じ scope になる。
    span は親の scope を継ぐ。
  - scope の外で生まれたレコードは、トークンなしで渡る。
- `init({ telemetry: { url, page } })` を渡すと、SDK が自分で wasm のログ（`logLevel` 以上）と
  span を observability サーバの `/api/client-telemetry` に送る（`core/telemetry.ts` の
  `ClientTelemetry`）。10秒ごとと `pagehide` のとき（`sendBeacon`）に、session のトークンごとに
  1レポートにまとめる。401 が返ったトークンのレコードは、以後トークンなしで送る。`onLog` と
  `onSpan` も渡せば両方に届く。
  - `ClientTelemetry` と TypeScript 側のログの sink は wasm を読み込む前に用意し、wasm の sink は
    読み込みの後に入れる。
    - 読み込みに失敗すると、`loadMoqt` が `wasm module failed to load` を ERROR でトークンなしに
      残す。これで wasm が動かないページも送られる。例: キャッシュされた古いページが再デプロイで
      消えた `.wasm` を指し、サーバーが HTML を返す。
    - `ClientTelemetry` はページに1つだけ作る。失敗した `init()` をやり直しても送信タイマーは
      増えない。
  - 送るたびに、`start` した `LivePlayer` の stats（`LiveStats`）を、その player の
    `trackNamespace`（`/` でつないだ namespace）と `mediaSource`（`source` の video track 名、
    `source` なしなら無し）とともに、player の session のトークンのレポートの `stats` に入れる。
    `RemoteBroadcast` の view も `LivePlayer` なので同じく送る。player は生成時に
    `registerPlayerStats` で `core/telemetry.ts` に自分を登録し、`dispose` で外す。`start` 前と
    `stop` 後は何も返さない。session のトークンはログと同じく `core/log.ts` の `sessionToken` で
    引くので、統計はログと同じ app に入る。`telemetry.playerStats: false` で止める（既定は送る）。
  - 同じく、音声を持つ `LocalBroadcast` の source ごとに `source.stats().audio`
    （`AudioPublishStats`）を、broadcast の `trackNamespace` と `mediaSource`（source 名）とともに、
    broadcast の session のトークンのレポートの `publisherStats` に入れる。source は生成時に
    `registerPublisherStats` で登録し、`removeSource` と `close` で外す。
  - `telemetry.stats` を渡すと、送るたびに呼び、返った stats
    （`{ trackNamespace?, mediaSource?, values }`）をトークンなしのレポートの `stats` に入れる。
    `LivePlayer` 以外の統計を足すためのもの。
    `redact` は stats には効かない。値はページが自分で作るので、隠すならそこで隠す。
  - `telemetry.url` は関数でもよい。送るたびに呼ぶので、ユーザーが選んだ relay に合わせて
    送り先を変えるページ（`examples/browser` の `startPageTelemetry`）に使う。
  - アプリ自身のログは、`reportLog(level, target, message, fields)` で渡したものだけを送る。
    SDK のログと同じ sink に、`logLevel` 以上だけが届く。コンソールには書かない。
  - 送り先を OTel Collector ではなく observability サーバにするのは、Collector が VPC の中でしか
    待ち受けず、ブラウザに Collector の資格情報を置けないから。トークンの検証、`app_id` の付与、
    レート制限はサーバが受け持つ（`docs/architecture/observability/architecture.md` の
    「Client telemetry」）。
- SDK の TypeScript 層のログも `onLog`（と `telemetry`）に届く（`core/log.ts`）。`sdkLog` は
  コンソールに書いたうえで、`init` が置いた sink に `LogRecord` で渡す。`target` は
  `moq-wasm::media::live_player` のようなモジュール名。
  - 呼び出しは最初の引数にログの対象の `MoqSession` を取り、sink にはその session の最新の
    トークン（wasm の `Session.token`、Rust 側のレコードと同じ `SessionScope` の値）を渡す。
    `MoqSession` は生成時に `bindLogToken` でトークンの読み方を `core/log.ts` に登録するので、
    トークンは `MoqSession` の API に出ない。session を持たないログ（`reportLog`、`PlayerControls`、
    catalog の解析）は `undefined` を渡し、トークンなしで渡る。`start` 前の `LivePlayer` も同じ。
  - worker（エンコーダとデコーダ）には sink がないので、`workerLog` が `{ type: 'log', … }` を
    postMessage し、ページ側の `onmessage` が `receiveWorkerLog` で、その worker の session を
    添えて `sdkLog` に渡す。
  - `LivePlayerCallbacks.onLog` と `PlayerControls` の `onLog` に渡すログは、ページが自分で
    表示するので、`forwardSdkLog` でコンソールに書かずに sink にだけ渡す。

形（TypeScript、抜粋）:

```ts
export function init(options?: {
  wasmUrl?: string | URL
  onLog?: (record: LogRecord, token: string | undefined) => void
  logLevel?: LogLevel                                          // 既定は 'warn'
  onSpan?: (record: SpanRecord, token: string | undefined) => void
  telemetry?: TelemetryOptions                                 // observability サーバへ送る
}): Promise<void>

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
export interface TelemetryOptions {
  url: string | (() => string)                                 // 関数なら送るたびに呼ぶ
  page: string
  redact?: (fields: LogRecord['fields']) => LogRecord['fields']
  playerStats?: boolean                                        // 既定は true: LivePlayer の stats を送る
  stats?: () => TelemetryStatsSample[]                         // 送るたびに呼ぶ
}
export interface TelemetryStatsSample {
  trackNamespace?: string                                      // 複数のトラックを再生するページだけ
  mediaSource?: string
  values: LiveStats
}
export function reportLog(level: LogLevel, target: string, message: string, fields?: Record<string, unknown>): void

export type MoqConnectOptions = {
  token?: string | (() => Promise<string>)       // CLIENT_SETUP の AUTHORIZATION TOKEN
  versions?: ('draft-14' | 'draft-18')[]         // 提示する draft。既定は draft-18、draft-14 の順
}

export class MoqSession {
  static connect(url: string, options?: MoqConnectOptions): Promise<MoqSession>
  readonly closed: Promise<CloseInfo>
  get isOpen(): boolean
  transportStats(): TransportStats
  subscribe(namespace: string[], track: string, options?: MoqSubscribeOptions): Promise<MoqSubscription>
  subscribeCurrentGroup(namespace: string[], track: string, options?: MoqSubscribeGroupStartOptions): Promise<MoqSubscription>  // SUBSCRIBE + joining FETCH
  subscribeGroupStart(namespace: string[], track: string, newGroupAfter: number, options?: MoqSubscribeGroupStartOptions): Promise<MoqSubscription>  // joining FETCH または NEW_GROUP_REQUEST
  fetch(namespace: string[], track: string, start: MoqLocation, end: MoqLocation): Promise<MoqFetch>
  trackStatus(namespace: string[], track: string): Promise<MoqLocation | undefined>
  announce(namespace: string[]): Promise<Announcement>          // PUBLISH_NAMESPACE
  publishTrack(namespace: string[], track: string, options?: PublishTrackOptions): Promise<TrackWriter>
  watchNamespaces(prefix: string[]): Promise<MoqNamespaceWatch>   // SUBSCRIBE_NAMESPACE; AsyncIterable<NamespaceEvent>
  close(): void
}

export class MoqSubscription implements AsyncIterable<ReceivedObject> {
  readonly largest: MoqLocation | undefined
  readonly done: Promise<PublishDone | undefined>              // PUBLISH_DONE
  groups(): AsyncIterable<MoqGroup>
  setForward(forward: boolean): Promise<void>
  setPriority(priority: number): Promise<void>
  requestNewGroup(largestGroup?: bigint): Promise<boolean>
  endBeforeGroup(groupId: bigint): Promise<void>
  close(): Promise<void>                                       // UNSUBSCRIBE
}

export class TrackWriter {                                     // wasm の型
  write(payload: Uint8Array, newGroup: boolean, locHeader: any): Promise<bigint | undefined>
  writeGroup(payload: Uint8Array): Promise<bigint | undefined>
  nextSubscriber(): Promise<boolean>
}
```

`subscribeCurrentGroup` と配信側の `TrackWriter` は、moqt にすでにある
`Subscriber::subscribe_current_group` と joining FETCH の範囲解決の上に作る。
`LivePlayer` の catalog 追従は `subscribeCurrentGroup(namespace, 'catalog')` を使う。

wasm 側の実装は `sdk/js/src/session.rs` の `Session`、`Subscription`、
`FetchStream`、`NamespaceWatch` で、iterator は `next()` の Promise として出す。
JS の `AsyncIterable` への包み直しと `groups()` は `core/session.ts` が持つ。
これらは `moqt::client` の同名の型（`FetchStream` は
`client::Fetch`）の wasm_bindgen の包みで、JsValue との変換、`SessionScope` による
telemetry の帰属、`RequestError` から draft-14 のコードを持つ `Error` への変換だけを
持つ。session のイベントの振り分けと、どこも引き受けないリクエストへの答え方は
`docs/architecture/moqt/architecture.md` の「`client` — the client session」にある。
`broadcast` と `mp4Broadcast` の `RelayConnection` は
`RelayConnection::with_client_session` で作り、`add_handler` で `Origin` より前に
並べる（外さない）。相手のリクエストへの答えは `moqt.client.subscribe`、
`moqt.client.publish`、`moqt.client.subscribe_namespace` の span に入る。`setLogSink`
が span の callback とともに `telemetry_report::continue_trace_from_traceparent` を
`moqt::set_trace_context_joiner` に渡すので、その span は相手の request span の子に
なる。`close()` は `moqt::Session::close_with_error` で "closed by the application"
を送り、待たずに戻る。

### 認証トークン

`token` は文字列か、文字列を返す非同期関数を受け取る。関数なら、最初の接続、GOAWAY に
従った新しい relay への接続、再接続のそれぞれで、CLIENT_SETUP を送る直前に呼ぶ。期限の
短い JWT を使うページは、ここで backend から新しいトークンを取る。呼ぶのは transport を
開いた後で、relay は開いてから 10 秒以内に CLIENT_SETUP が届かない接続を閉じる。関数は
その間に答える。これは wasm ではなく
moqt の役目である。`ClientConfig::authorization_token` は、固定値と、接続ごとに呼ばれる
provider の両方を表す型で、`Session` が新しい `Connection` を開くたびに provider
を呼ぶ。Rust の publisher（ingest-gateway など）も同じ口を使う。

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
- **再試行。** 0.5秒から始めて最大10秒まで間隔を倍にしながら、60秒（relay の
  キャッシュ保持時間に合わせた固定値）まで試す。その間、配信側の object は
  `Origin` に溜まる。諦めたら
  `session.closed` が解決し、すべての handle が終わる（iterator は終わり、書き込みは
  reject する）。
- **認証エラー。** 再接続先が SETUP で認証エラーを返したら、トークンを取り直して1回
  だけ再試行し、それでも失敗したら諦める。

## bundler 互換

パッケージは TypeScript ではなくビルド済みの JavaScript を配る。
- **wasm**: `wasm-pack --target web` の glue は、`moqt_bg.wasm` を
  `new URL('moqt_bg.wasm', import.meta.url)` で解決する。Vite、webpack 5、Rollup
  （URL plugin 付き）、Parcel はこれを書き換える。CDN や、書き換えない bundler には
  `init({ wasmUrl })` で対応する。
- **worker**: decoder と encoder の worker は、ファイルごとに
  `new Worker(new URL('<相対パス>', import.meta.url), { type: 'module' })` で起動
  する。URL を上書きする option は無い。パッケージは Vite の app build（library mode は wasm と worker を data URL に
  埋め込む）で作り、出力の URL を `new URL('<相対パス>', import.meta.url)` の形に
  そろえる。
- **AudioWorklet**: Safari の音声 capture の module は、ソースを文字列として取り込み、
  Blob URL から `addModule` する。小さい asset は利用者の bundler が data URL に埋め込む
  ので、asset にはしない。
- **CSS**: JS は CSS を読み込まない。`LivePlayer` を出すページは
  `moq-wasm/media/player.css`、`PlayerControls` も出すページは
  `moq-wasm/media/controls.css` を読み込む。
- `init()` は single-flight で、何度呼んでも同じ結果になる。`MoqSession.connect` は
  内部で `loadMoqt()` を呼んで wasm を読み込む。
- パッケージに実行時の依存は無い。React は examples に残す。

## バージョニング

- パッケージのバージョンは、crate のバージョンとは独立にリポジトリのリリースに合わせ、
  semver に従う。1.0 までは minor でも API を壊してよく、壊したものはすべて
  changelog に書く。
- 公開範囲は `moq-wasm`、`moq-wasm/media`、`moq-wasm/element`、`moq-wasm/recording` が
  export するものと CSS の subpath で、
  `internal/` 以下は含まない。session がネゴシエートする draft のバージョンも API の
  約束の一部とし、changelog に書く。
- wasm、glue、TypeScript は1つの成果物なので、ページがそれらのバージョンを混ぜる
  ことはない。

## 決定事項

1. **codec: 映像は H.264、H.265、VP8、VP9、AV1。音声は配信が Opus、再生が Opus と AAC。** 既定は
   `avc1.640028` と `opus`。`addSource` の `encoding.codec` で選び、ブラウザが encode できない
   codec を指定したら reject する（「配信を `crates/broadcast` に寄せる」の変更6）。
   `encoding.codec` は名前（`'h264'`、`'h265'`、`'vp8'`、`'vp9'`、`'av1'`）か WebCodecs の codec
   文字列を取る。名前のときは SDK が profile を決める（H.264 は High、H.265 は Main、VP9 は
   Profile 0、AV1 は Main）。H.264 の level はトラックごとの大きさと frame rate から上げる。
   VP9 と AV1 の level は固定。H.265 の level は encoder が大きさに合わせてビットストリームに
   書き、catalog の `codec` はそこから作る。H.265 は H.264 と同じく Annex-B で送り、parameter
   set は keyframe に in-band で載る。Chrome は H.265 をハードウェアでしか encode / decode
   できないので、player は H.265 のときだけ decoder の `hardwareAcceleration` を
   `'no-preference'` にする（ほかの codec は `'prefer-software'`）。
2. **認証トークン: 文字列か、更新用の非同期関数。** 関数は、接続、GOAWAY による移動、
   再接続のたびに呼ぶ（「core API」の「認証トークン」）。
3. **再接続: 自動。** GOAWAY 無しの切断でも session が接続し直し、handle はそのまま
   使える。relay のキャッシュにある範囲は FETCH で埋め、上限（既定60秒）を過ぎたら
   諦める（「core API」の「再接続」）。
4. **catalog: `addSource` で指定できる。** track ごとの label、role、rendition を
   渡せる。独自フィールドは、使う用途が出るまで見送る（変更7）。
5. **Audio Level: 含める。** `createBroadcast()` は音声の object ごとにレベルを計算し、LOC の
   Audio Level property として送る（変更5）。
6. **CMAF の兄弟 track: 作らない。** ブラウザの配信は CMAF を作らない。他の publisher（ingest-gateway など）の既定は変えない
   （変更8）。
7. **パッケージ名: `moq-wasm`。** `package.json` の `name` と import パスに使う。
   3言語の SDK の名前をリポジトリ名にそろえる（`docs/architecture/moqt/sdk.md`）。
8. **対応ブラウザ: Chrome、Edge、Safari。** Safari の違いは「Safari 対応」に書いた
   代替経路で吸収する。
9. **プレイヤー UI: `PlayerControls` も含める。** コンストラクタで container、player、
   ログ用の callback を受け取り、ページが `onStateChange` から `render()` を呼ぶ。
   見た目は `controls.css` に閉じ、class 名と CSS 変数（色、サイズ）を公開範囲に含める。
   markup の構造は含めない。`dispose()` で、追加した DOM とイベントの購読を外す。
10. **安定性: semver。1.0 までは minor で壊してよい。** core API（`MoqSession`、
    `MoqSubscription`、`TrackWriter`）を先に 1.0 にする。`moq-wasm/media` は、examples
    以外のアプリが1つ、それで配信と再生をするまで 1.0 未満のままにする。両方を
    1パッケージで出すので、media API が 1.0 未満であることは、バージョンを分けず、
    doc comment と README に書く。
11. **マルチコーデック: 1つの altGroup に別 codec の rendition を並べる。** MSF の altGroup
    （draft-ietf-moq-msf-02 §5.2.13）は selection property の違う同じ内容の track で、codec
    もその1つなので、たとえば H.265 の映像と H.264 の rendition を同じ altGroup に置く。
    altGroup は time-aligned（§4.2）なので、encoder worker が全 rendition で同じフレームを
    keyframe にする今の仕組みのまま codec が混ざってよい。player は altGroup の中から、
    ブラウザが decode できる track（`VideoDecoder.isConfigSupported`）だけを候補にし、
    同じ幅と高さの track が複数残ったら bitrate の一番低いものだけを残す
    （`playableVideoTracks.ts`）。decode できる track が1つも無ければ全部を残し、decoder に
    理由を報告させる。codec の違う rendition への切り替えは、その rendition の最初の
    keyframe で decoder を `configure` し直す（level だけの違いでは設定し直さない）。
    これより前の SDK の player は codec を見ずに ladder を作るので、decode できない
    rendition を選んで止まりうる。配信側は rendition ごとに `codec` を指定する。ブラウザが
    encode できない codec の rendition は、`onError` に報告して catalog に載せず、映像と
    ほかの rendition はそのまま送る。

## 配布

最初は npm レジストリに公開せず、`npm pack` で作った tarball を GitHub Release に添付して
配る。API を壊してもよい間（1.0 未満）は、試してくれる利用者と直接やり取りしながら進める
ので、レジストリの利点（検索、範囲指定による更新、CDN）はまだ要らない。

利用者は、`package.json` に tarball の URL を書いて `npm install` する。中身はビルド
済みなので、Rust や wasm-pack は要らない。lock ファイルには URL とハッシュが記録される。

```json
{
  "dependencies": {
    "moq-wasm": "https://github.com/<owner>/<repo>/releases/download/js-sdk-v0.1.0/moq-wasm-0.1.0.tgz"
  }
}
```

リリースの手順:

1. パッケージの `package.json` の version を上げ、`CHANGELOG.md` にその version の
   節を足す。節の中身は、前の tag から後にマージされた PR の本文の「リリースノート」
   の節を集めて作る（`.agents/skills/release-sdk/SKILL.md`）。並行する PR が同じ
   `CHANGELOG.md` に書くと衝突するので、PR は `CHANGELOG.md` を変えず、載せる英文を
   PR の本文に書く。
2. `js-sdk-v<version>` の tag を push する。crate や Python SDK とは別に版を刻むので、
   tag には `js-sdk-` を付ける。
3. CI がその tag で wasm と TS をビルドし、`npm pack` の tarball を作り、同じ名前の
   GitHub Release に添付する。changelog の該当部分を Release の本文にする。

3 は `.github/workflows/sdk-release.yml` が行う。tag がパッケージの version を指して
いなければ止まり、`CHANGELOG.md` にその version の節が無くても止まる。添付する前に、CI と同じ
`check:pack` を通す。

tarball 配布の制約: `^0.1.0` のような範囲指定、`npm update`、Dependabot や Renovate に
よる更新は効かず、利用者は URL を書き換えて更新する。jsDelivr や unpkg からも使えない。

## 範囲外

- **npm への公開。** core API が 1.0 になったときに改めて決める。それまでに名前
  （`moq-wasm`）が npm で空いているかを確かめる。採らなかった配布方法は次のとおり。
  - **Git の URL（`github:<owner>/<repo>#<tag>`）。** npm はリポジトリのルートの
    `package.json` しか見ないので、サブディレクトリ（`sdk/js/js/`）にある
    パッケージは入れられない。入れられたとしても、インストール時の `prepare` で
    wasm のビルドが走り、利用者に Rust と wasm-pack が要る。
  - **GitHub Packages。** public なパッケージでも、インストールに認証トークンが要る。
