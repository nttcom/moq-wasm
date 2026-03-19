declare const __CERT_HASH_BASE64__: string;

import {
	FilterType,
	FullTrackName,
	GroupOrder,
	MOQtailClient,
	SubscribeError,
} from "moqtail";

const videoEl = document.getElementById("video") as HTMLVideoElement;
const nsInput = document.getElementById("nsInput") as HTMLInputElement;
const trackInput = document.getElementById("trackInput") as HTMLInputElement;
const startBtn = document.getElementById("startBtn") as HTMLButtonElement;
const logEl = document.getElementById("log") as HTMLDivElement;

// publisher の generate_namespace() と同じ形式でデフォルト値を設定
const now = new Date();
const hh = String(now.getHours()).padStart(2, "0");
const mm = String(now.getMinutes()).padStart(2, "0");
nsInput.value = `live-${hh}${mm}`;

const MAX_LOG_LINES = 100;
const logLines: string[] = [];
function log(msg: string) {
	console.log(msg);
	logLines.push(msg);
	if (logLines.length > MAX_LOG_LINES) logLines.shift();
	logEl.textContent = logLines.join("\n");
	logEl.scrollTop = logEl.scrollHeight;
}

function decodeCertHash(base64: string): Uint8Array {
	const binary = atob(base64);
	const bytes = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i++) {
		bytes[i] = binary.charCodeAt(i);
	}
	return bytes;
}

startBtn.addEventListener("click", async () => {
	startBtn.disabled = true;
	nsInput.disabled = true;
	trackInput.disabled = true;

	const namespace = nsInput.value.trim();
	const trackName = trackInput.value.trim();
	if (!namespace || !trackName) {
		log("Error: namespace and track name are required");
		startBtn.disabled = false;
		nsInput.disabled = false;
		trackInput.disabled = false;
		return;
	}

	try {
		// Connect to relay via WebTransport
		const url = "https://localhost:4433";
		const certHash = decodeCertHash(__CERT_HASH_BASE64__);
		log(`Connecting to ${url}...`);

		const client = await MOQtailClient.new({
			url,
			supportedVersions: [0xff00000e],
			enableDatagrams: false,
			transportOptions: {
				serverCertificateHashes: [
					{
						algorithm: "sha-256",
						value: certHash.buffer,
					},
				],
			},
			callbacks: {
				onSessionTerminated: (reason) => {
					log(`Session terminated: ${reason}`);
				},
				onMessageSent: (msg) => {
					console.log(`[sent] ${msg.constructor.name}`, msg);
				},
				onMessageReceived: (msg) => {
					console.log(`[recv] ${msg.constructor.name}`, msg);
				},
			},
		});
		log("Connected (SETUP done)");

		// Subscribe to track
		const fullTrackName = FullTrackName.tryNew(namespace, trackName);
		log(`Subscribing to ${namespace}/${trackName}...`);

		const result = await client.subscribe({
			fullTrackName,
			priority: 128,
			groupOrder: GroupOrder.Ascending,
			forward: true,
			filterType: FilterType.LatestObject,
		});

		if (result instanceof SubscribeError) {
			log(`Subscribe error: ${result}`);
			return;
		}

		log(`Subscribed (requestId=${result.requestId})`);
		const stream = result.stream;

		// Read objects from the stream, initialize MSE on first init segment
		const reader = stream.getReader();
		let initWritten = false;
		let currentObjectId = 0n;
		let sourceBuffer: SourceBuffer | null = null;
		let mediaSource: MediaSource | null = null;

		const appendQueue: Uint8Array[] = [];
		let appending = false;

		function seekToLiveEdge() {
			if (!sourceBuffer || !videoEl.buffered.length) return;
			const end = videoEl.buffered.end(videoEl.buffered.length - 1);
			if (end - videoEl.currentTime > 2) {
				videoEl.currentTime = end - 0.5;
			}
		}

		function appendNext() {
			if (!sourceBuffer || appending || appendQueue.length === 0) return;
			if (sourceBuffer.updating) return;
			appending = true;
			const data = appendQueue.shift();
			if (!data) return;
			try {
				sourceBuffer.appendBuffer(data);
			} catch (e) {
				log(`appendBuffer error: ${e}`);
				appending = false;
			}
		}

		function enqueueBuffer(data: Uint8Array) {
			appendQueue.push(data);
			appendNext();
		}

		// Extract avcC codec string from init segment
		// avcC box layout: [size(4)] [type="avcC"(4)] [configurationVersion(1)] [profileIdc(1)] [constraintFlags(1)] [levelIdc(1)] ...
		function extractCodecString(initSegment: Uint8Array): string {
			// Search for "avcC" box type
			for (let i = 0; i < initSegment.length - 11; i++) {
				if (
					initSegment[i] === 0x61 &&
					initSegment[i + 1] === 0x76 &&
					initSegment[i + 2] === 0x63 &&
					initSegment[i + 3] === 0x43
				) {
					// avcC type found at i, box data starts after type
					// i+4 = configurationVersion (should be 1)
					// i+5 = profileIdc
					// i+6 = constraintFlags
					// i+7 = levelIdc
					const profileIdc = initSegment[i + 5];
					const constraintFlags = initSegment[i + 6];
					const levelIdc = initSegment[i + 7];
					const codec = `avc1.${profileIdc.toString(16).padStart(2, "0")}${constraintFlags.toString(16).padStart(2, "0")}${levelIdc.toString(16).padStart(2, "0")}`;
					return codec;
				}
			}
			return "avc1.42001f"; // fallback
		}

		async function initMSE(initSegment: Uint8Array) {
			const codec = extractCodecString(initSegment);
			const mimeType = `video/mp4; codecs="${codec}"`;
			log(`Detected codec: ${codec}`);

			if (!MediaSource.isTypeSupported(mimeType)) {
				log(`Error: ${mimeType} is not supported`);
				return false;
			}

			mediaSource = new MediaSource();
			videoEl.src = URL.createObjectURL(mediaSource);

			await new Promise<void>((resolve) => {
				mediaSource?.addEventListener("sourceopen", () => resolve(), {
					once: true,
				});
			});

			sourceBuffer = mediaSource.addSourceBuffer(mimeType);
			sourceBuffer.addEventListener("updateend", () => {
				appending = false;
				seekToLiveEdge();
				appendNext();
			});
			sourceBuffer.addEventListener("error", (e) => {
				log(`SourceBuffer error: ${e}`);
			});

			videoEl.addEventListener("error", () => {
				const err = videoEl.error;
				log(`Video error: code=${err?.code} message=${err?.message}`);
			});

			log("MSE initialized");
			enqueueBuffer(initSegment);
			return true;
		}

		while (true) {
			const { done, value: obj } = await reader.read();
			if (done) {
				log("Stream ended");
				break;
			}

			const payload = obj.payload;
			if (!payload || payload.byteLength === 0) continue;

			const location = obj.location;
			const groupId = location?.group ?? 0n;
			const objectId = location?.object ?? currentObjectId;
			currentObjectId = objectId + 1n;

			if (objectId === 0n) {
				// Object 0: init segment
				if (!initWritten) {
					log(
						`Init segment received (${payload.byteLength} bytes, group=${groupId})`,
					);
					const ok = await initMSE(new Uint8Array(payload));
					if (!ok) break;
					initWritten = true;
				}
			} else {
				// Object 1+: media segment
				if (!initWritten) continue; // skip until init is written
				log(`Media segment (${payload.byteLength} bytes, group=${groupId})`);
				enqueueBuffer(new Uint8Array(payload));
			}
		}
	} catch (err) {
		log(`Error: ${err}`);
		console.error(err);
		startBtn.disabled = false;
		nsInput.disabled = false;
		trackInput.disabled = false;
	}
});
