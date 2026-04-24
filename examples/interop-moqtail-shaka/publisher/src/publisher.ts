import {
	MediaStreamAudioTrackSource,
	MediaStreamVideoTrackSource,
	Mp4OutputFormat,
	NullTarget,
	Output,
} from "mediabunny";
import {
	FullTrackName,
	LiveTrackSource,
	Location,
	type MOQtailClient,
	MoqtObject,
	ObjectForwardingPreference,
	SubscribeOk,
	Tuple,
} from "moqtail";
import { createClient } from "./connection";
import {
	AUDIO_BITRATE,
	AUDIO_CHANNELS,
	AUDIO_CODEC,
	AUDIO_SAMPLERATE,
	buildCatalog,
	defaultNamespace,
	KEYFRAME_INTERVAL,
	VIDEO_BITRATE,
	VIDEO_CODEC,
	VIDEO_FRAMERATE,
	VIDEO_HEIGHT,
	VIDEO_WIDTH,
} from "./constants";
import { log } from "./log";

const preview = document.getElementById("preview") as HTMLVideoElement;
const connectBtn = document.getElementById("connectBtn") as HTMLButtonElement;
const publishNsBtn = document.getElementById(
	"publishNsBtn",
) as HTMLButtonElement;
const startBtn = document.getElementById("startBtn") as HTMLButtonElement;
const nsInput = document.getElementById("nsInput") as HTMLInputElement;

nsInput.value = defaultNamespace();

let client: MOQtailClient;
let mediaStream: MediaStream;
let namespacePath: string;

const CATALOG_TRACK_ALIAS = 3n;

let catalogController: ReadableStreamDefaultController<MoqtObject> | null =
	null;
let catalogTrackName: FullTrackName | null = null;
let catalogGroupId = 0n;

// Step 1: Get camera/mic, connect to relay
connectBtn.addEventListener("click", async () => {
	connectBtn.disabled = true;
	nsInput.disabled = true;

	namespacePath = nsInput.value.trim();
	if (!namespacePath) {
		log("Error: namespace is empty");
		connectBtn.disabled = false;
		nsInput.disabled = false;
		return;
	}

	try {
		log("Requesting camera/mic...");
		mediaStream = await navigator.mediaDevices.getUserMedia({
			video: {
				width: VIDEO_WIDTH,
				height: VIDEO_HEIGHT,
				frameRate: VIDEO_FRAMERATE,
			},
			audio: { sampleRate: AUDIO_SAMPLERATE, channelCount: AUDIO_CHANNELS },
		});
		preview.srcObject = mediaStream;
		log("Camera/mic acquired");

		log("Connecting to relay...");
		client = await createClient({
			// When relay confirms a catalog subscription (SubscribeOk),
			// send the catalog JSON so the subscriber can discover tracks.
			onMessageSent: (msg) => {
				if (
					msg instanceof SubscribeOk &&
					msg.trackAlias === CATALOG_TRACK_ALIAS
				) {
					if (!catalogController || !catalogTrackName) return;

					const catalog = buildCatalog(namespacePath);
					const payload = new TextEncoder().encode(JSON.stringify(catalog));

					catalogGroupId++;
					catalogController.enqueue(
						MoqtObject.newWithPayload(
							catalogTrackName, // fullTrackName
							new Location(catalogGroupId, 0n), // location
							0, // publisherPriority
							ObjectForwardingPreference.Subgroup, // forwardingPreference
							0n, // subgroupId
							null, // extensions
							payload, // payload
						),
					);
					log(
						`Catalog enqueued (group=${catalogGroupId}, ${payload.byteLength} bytes)`,
					);
				}
			},
		});
		log("Connected to relay (SETUP done)");

		publishNsBtn.disabled = false;
	} catch (err) {
		log(`Error: ${err}`);
		console.error(err);
		connectBtn.disabled = false;
		nsInput.disabled = false;
	}
});

// Step 2: Register namespace on relay so subscribers can discover it
publishNsBtn.addEventListener("click", async () => {
	publishNsBtn.disabled = true;

	try {
		log(`Publishing namespace: ${namespacePath}`);
		await client.publishNamespace(Tuple.fromUtf8Path(namespacePath));
		log("Namespace published");

		startBtn.disabled = false;
	} catch (err) {
		log(`Error: ${err}`);
		console.error(err);
		publishNsBtn.disabled = false;
	}
});

// Step 3: Register tracks, encode camera/mic into fMP4, and publish via MoQT
startBtn.addEventListener("click", async () => {
	startBtn.disabled = true;

	const videoTrackName = FullTrackName.tryNew(namespacePath, "video");
	const audioTrackName = FullTrackName.tryNew(namespacePath, "audio");
	catalogTrackName = FullTrackName.tryNew(namespacePath, "catalog");

	try {
		let videoGroupId = 0n;
		let audioGroupId = 0n;

		// Create ReadableStreams that feed MoQT objects to the relay.
		// Each controller lets us enqueue objects from the encoder callbacks.
		let videoController: ReadableStreamDefaultController<MoqtObject>;
		const videoMoqStream = new ReadableStream<MoqtObject>({
			start(c) {
				videoController = c;
			},
		});

		let audioController: ReadableStreamDefaultController<MoqtObject>;
		const audioMoqStream = new ReadableStream<MoqtObject>({
			start(c) {
				audioController = c;
			},
		});

		const catalogMoqStream = new ReadableStream<MoqtObject>({
			start(c) {
				catalogController = c;
			},
		});

		// Register tracks on the client so the relay knows what we publish
		client.addOrUpdateTrack({
			fullTrackName: videoTrackName,
			forwardingPreference: ObjectForwardingPreference.Subgroup,
			trackSource: { live: new LiveTrackSource(videoMoqStream) },
			publisherPriority: 0,
			trackAlias: 1n,
		});

		client.addOrUpdateTrack({
			fullTrackName: audioTrackName,
			forwardingPreference: ObjectForwardingPreference.Subgroup,
			trackSource: { live: new LiveTrackSource(audioMoqStream) },
			publisherPriority: 0,
			trackAlias: 2n,
		});

		client.addOrUpdateTrack({
			fullTrackName: catalogTrackName,
			forwardingPreference: ObjectForwardingPreference.Subgroup,
			trackSource: { live: new LiveTrackSource(catalogMoqStream) },
			publisherPriority: 0,
			trackAlias: CATALOG_TRACK_ALIAS,
		});
		log("Tracks registered");

		// Build an fMP4 encoder pipeline.
		// mediabunny emits moof and mdat boxes separately;
		// we concatenate them into a single fragment (moof+mdat) per callback.
		const createFmp4Pipeline = (opts: {
			minimumFragmentDuration: number;
			onFragment: (data: Uint8Array) => void;
		}) => {
			let currentMoof: Uint8Array | null = null;

			const format = new Mp4OutputFormat({
				fastStart: "fragmented",
				minimumFragmentDuration: opts.minimumFragmentDuration,
				onMoof(data) {
					currentMoof = new Uint8Array(data);
				},
				onMdat(data) {
					if (!currentMoof) return;
					const moof = currentMoof;
					currentMoof = null;

					const fragment = new Uint8Array(moof.byteLength + data.byteLength);
					fragment.set(moof, 0);
					fragment.set(new Uint8Array(data), moof.byteLength);
					opts.onFragment(fragment);
				},
			});

			return new Output({ format, target: new NullTarget() });
		};

		// Video pipeline: encode each frame as a fragment, enqueue as MoQT object
		const videoOutput = createFmp4Pipeline({
			minimumFragmentDuration: KEYFRAME_INTERVAL - 0.1, // margin for encoder timing
			onFragment(fragment) {
				videoGroupId++;
				videoController.enqueue(
					MoqtObject.newWithPayload(
						videoTrackName, // fullTrackName
						new Location(videoGroupId, 0n), // location
						0, // publisherPriority
						ObjectForwardingPreference.Subgroup, // forwardingPreference
						0n, // subgroupId
						null, // extensions
						fragment, // payload
					),
				);
			},
		});

		const videoTrack = mediaStream.getVideoTracks()[0];
		const videoSource = new MediaStreamVideoTrackSource(videoTrack, {
			codec: VIDEO_CODEC,
			bitrate: VIDEO_BITRATE,
			keyFrameInterval: KEYFRAME_INTERVAL,
		});
		videoOutput.addVideoTrack(videoSource);

		// Audio pipeline: batch samples per keyframe interval, enqueue as MoQT object
		const audioOutput = createFmp4Pipeline({
			minimumFragmentDuration: KEYFRAME_INTERVAL,
			onFragment(fragment) {
				audioGroupId++;
				audioController.enqueue(
					MoqtObject.newWithPayload(
						audioTrackName, // fullTrackName
						new Location(audioGroupId, 0n), // location
						0, // publisherPriority
						ObjectForwardingPreference.Subgroup, // forwardingPreference
						0n, // subgroupId
						null, // extensions
						fragment, // payload
					),
				);
			},
		});

		const audioTrack = mediaStream.getAudioTracks()[0];
		const audioSource = new MediaStreamAudioTrackSource(audioTrack, {
			codec: AUDIO_CODEC,
			bitrate: AUDIO_BITRATE,
		});
		audioOutput.addAudioTrack(audioSource);

		videoSource.errorPromise.catch((e) =>
			console.error("Video source error:", e),
		);
		audioSource.errorPromise.catch((e) =>
			console.error("Audio source error:", e),
		);
		await Promise.all([videoOutput.start(), audioOutput.start()]);
	} catch (err) {
		log(`Error: ${err}`);
		console.error(err);
		startBtn.disabled = false;
	}
});
