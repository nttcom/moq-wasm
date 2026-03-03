import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO } from '../const'
import { getFormElement } from '../utils'
import { extractCatalogVideoTracks, extractCatalogAudioTracks, type MediaCatalogTrack } from '../catalog'

const moqtClient = new MoqtClientWrapper()

let handlersInitialized = false
let catalogVideoTracks: MediaCatalogTrack[] = []
let catalogAudioTracks: MediaCatalogTrack[] = []
let selectedVideoTrackName: string | null = null

// MSE state
let mediaSource: MediaSource | null = null

let videoSourceBuffer: SourceBuffer | null = null
let videoInitReceived = false
const videoAppendQueue: Uint8Array[] = []
let isVideoAppending = false

let audioSourceBuffer: SourceBuffer | null = null
let audioInitReceived = false
const audioAppendQueue: Uint8Array[] = []
let isAudioAppending = false

const PLAYBACK_BUFFER_THRESHOLD = 1.0 // バッファがこの秒数を超えたら再生開始（1フラグメント分）
let playbackStarted = false

// --- Helpers ---

const toBigUint64Array = (value: string): BigUint64Array => {
	const values = value
		.split(',')
		.map((part) => part.trim())
		.filter((part) => part.length > 0)
		.map((part) => BigInt(part))
	return new BigUint64Array(values)
}

const parseTrackNamespace = (value: string): string[] => {
	return value
		.split('/')
		.map((part) => part.trim())
		.filter((part) => part.length > 0)
}

// --- Catalog ---

const setCatalogTrackStatus = (text: string): void => {
	const status = document.getElementById('catalog-track-status')
	if (!status) {
		return
	}
	const normalized = text.trim()
	status.textContent = normalized
	status.style.display = normalized.length > 0 ? '' : 'none'
}

const getCatalogTrackSelect = (): HTMLSelectElement | null => {
	return document.getElementById('selected-video-track') as HTMLSelectElement | null
}

const formatCatalogTrackLabel = (track: MediaCatalogTrack): string => {
	const resolution =
		typeof track.width === 'number' && typeof track.height === 'number' ? ` (${track.width}x${track.height})` : ''
	return `${track.label}${resolution}`
}

const renderCatalogTrackSelect = (): boolean => {
	const select = getCatalogTrackSelect()
	if (!select) {
		return false
	}
	select.innerHTML = ''

	if (!catalogVideoTracks.length) {
		const option = document.createElement('option')
		option.value = ''
		option.textContent = 'Catalog video tracks are not loaded yet'
		option.disabled = true
		option.selected = true
		select.appendChild(option)
		selectedVideoTrackName = null
		return false
	}

	if (!selectedVideoTrackName || !catalogVideoTracks.some((track) => track.name === selectedVideoTrackName)) {
		selectedVideoTrackName = catalogVideoTracks[0].name
	}

	for (const track of catalogVideoTracks) {
		const option = document.createElement('option')
		option.value = track.name
		option.textContent = formatCatalogTrackLabel(track)
		option.selected = track.name === selectedVideoTrackName
		select.appendChild(option)
	}
	return true
}

const renderCatalogTracks = (): void => {
	const hasVideoTracks = renderCatalogTrackSelect()

	if (!hasVideoTracks) {
		setCatalogTrackStatus('')
		return
	}
	setCatalogTrackStatus(`Catalog loaded: video=${catalogVideoTracks.length} audio=${catalogAudioTracks.length}`)
}

const setupCatalogSelectionHandler = (): void => {
	const videoSelect = getCatalogTrackSelect()
	if (videoSelect) {
		videoSelect.addEventListener('change', () => {
			const value = videoSelect.value.trim()
			selectedVideoTrackName = value.length > 0 ? value : null
		})
	}
}

const setupCatalogCallbacks = (trackAlias: bigint): void => {
	moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
		const payload = new TextDecoder().decode(new Uint8Array(subgroupStreamObject.objectPayload))
		try {
			const parsed = parse_msf_catalog_json(payload)
			catalogVideoTracks = extractCatalogVideoTracks(parsed)
			catalogAudioTracks = extractCatalogAudioTracks(parsed)
			renderCatalogTracks()
		} catch (error) {
			console.error('[CmafSubscriber] failed to parse catalog', error)
			setCatalogTrackStatus('Catalog parse failed')
		}
	})
}

// --- Overhead display ---

const updateOverheadDisplay = (type: 'video' | 'audio', init: number, moof: number, mdat: number, overhead: string): void => {
	const setText = (id: string, text: string) => {
		const el = document.getElementById(id)
		if (el) {
			el.textContent = text
		}
	}
	setText(`oh-${type}-init`, `${init} bytes`)
	setText(`oh-${type}-moof`, `${moof} bytes`)
	setText(`oh-${type}-mdat`, `${mdat} bytes`)
	setText(`oh-${type}-pct`, `${overhead}%`)
}

// --- MSE helpers ---

const buildVideoMimeType = (codec: string): string => {
	return `video/mp4; codecs="${codec}"`
}

const buildAudioMimeType = (codec: string): string => {
	return `audio/mp4; codecs="${codec}"`
}

const appendToVideo = (data: Uint8Array): void => {
	videoAppendQueue.push(data)
	flushVideoQueue()
}

const flushVideoQueue = (): void => {
	if (isVideoAppending || !videoSourceBuffer || videoAppendQueue.length === 0) {
		return
	}
	isVideoAppending = true
	const data = videoAppendQueue.shift()!
	try {
		videoSourceBuffer.appendBuffer(data)
	} catch (e) {
		isVideoAppending = false
		console.error('[CmafSubscriber] video appendBuffer failed', e)
	}
}

const appendToAudio = (data: Uint8Array): void => {
	audioAppendQueue.push(data)
	flushAudioQueue()
}

const flushAudioQueue = (): void => {
	if (isAudioAppending || !audioSourceBuffer || audioAppendQueue.length === 0) {
		return
	}
	isAudioAppending = true
	const data = audioAppendQueue.shift()!
	try {
		audioSourceBuffer.appendBuffer(data)
	} catch (e) {
		isAudioAppending = false
		console.error('[CmafSubscriber] audio appendBuffer failed', e)
	}
}

const setupMediaSource = (videoCodec: string, audioCodec: string): void => {
	mediaSource = new MediaSource()
	const videoElement = document.getElementById('video') as HTMLVideoElement
	videoElement.src = URL.createObjectURL(mediaSource)

	mediaSource.addEventListener('sourceopen', () => {
		const videoMime = buildVideoMimeType(videoCodec)
		const audioMime = buildAudioMimeType(audioCodec)
		console.info('[CmafSubscriber] MediaSource open, adding SourceBuffers', { videoMime, audioMime })

		videoSourceBuffer = mediaSource!.addSourceBuffer(videoMime)
		videoSourceBuffer.mode = 'sequence'
		videoSourceBuffer.addEventListener('updateend', () => {
			isVideoAppending = false
			if (!playbackStarted && videoSourceBuffer!.buffered.length > 0) {
				const buffered = videoSourceBuffer!.buffered.end(0) - videoSourceBuffer!.buffered.start(0)
				if (buffered >= PLAYBACK_BUFFER_THRESHOLD) {
					playbackStarted = true
					videoElement.play()
					console.info('[CmafSubscriber] playback started', { buffered: buffered.toFixed(2) })
				}
			}
			flushVideoQueue()
		})
		videoSourceBuffer.addEventListener('error', (e) => {
			console.error('[CmafSubscriber] video SourceBuffer error', e)
		})

		audioSourceBuffer = mediaSource!.addSourceBuffer(audioMime)
		audioSourceBuffer.mode = 'sequence'
		audioSourceBuffer.addEventListener('updateend', () => {
			isAudioAppending = false
			flushAudioQueue()
		})
		audioSourceBuffer.addEventListener('error', (e) => {
			console.error('[CmafSubscriber] audio SourceBuffer error', e)
		})

		flushVideoQueue()
		flushAudioQueue()
	})
}

// --- MoQ Object handlers ---

const setupVideoObjectCallbacks = (trackAlias: bigint): void => {
	videoInitReceived = false
	let lastVideoGroupId: bigint | null = null
	let videoInitSize = 0

	moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
		const raw = new Uint8Array(subgroupStreamObject.objectPayload)
		const objectId = subgroupStreamObject.objectId
		const payload = raw

		if (_groupId !== lastVideoGroupId) {
			lastVideoGroupId = _groupId
		}

		if (objectId === 0n) {
			videoInitSize = payload.byteLength
			if (!videoInitReceived) {
				console.info('[CmafSubscriber] video init segment received', { byteLength: payload.byteLength })
				videoInitReceived = true
				appendToVideo(payload)
			}
			return
		}

		const moofSize = new DataView(payload.buffer, payload.byteOffset, 4).getUint32(0)
		const mdatSize = payload.byteLength - moofSize
		const headerBytes = videoInitSize + moofSize
		const overhead = (headerBytes / (headerBytes + mdatSize) * 100).toFixed(2)
		updateOverheadDisplay('video', videoInitSize, moofSize, mdatSize, overhead)
		appendToVideo(payload)
	})
}

const setupAudioObjectCallbacks = (trackAlias: bigint): void => {
	audioInitReceived = false
	let lastAudioGroupId: bigint | null = null
	let audioInitSize = 0

	moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
		const raw = new Uint8Array(subgroupStreamObject.objectPayload)
		const objectId = subgroupStreamObject.objectId
		const payload = raw

		if (_groupId !== lastAudioGroupId) {
			lastAudioGroupId = _groupId
		}

		if (objectId === 0n) {
			audioInitSize = payload.byteLength
			if (!audioInitReceived) {
				console.info('[CmafSubscriber] audio init segment received', { byteLength: payload.byteLength })
				audioInitReceived = true
				appendToAudio(payload)
			}
			return
		}

		const moofSize = new DataView(payload.buffer, payload.byteOffset, 4).getUint32(0)
		const mdatSize = payload.byteLength - moofSize
		const headerBytes = audioInitSize + moofSize
		const overhead = (headerBytes / (headerBytes + mdatSize) * 100).toFixed(2)
		updateOverheadDisplay('audio', audioInitSize, moofSize, mdatSize, overhead)
		appendToAudio(payload)
	})
}

// --- Button handlers ---

const sendSetupButtonClickHandler = (): void => {
	const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
	sendSetupBtn.addEventListener('click', async () => {
		const form = getFormElement()
		const versions = toBigUint64Array('0xff00000A')
		const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
		await moqtClient.sendSetupMessage(versions, maxSubscribeId)
	})
}

const sendCatalogSubscribeButtonClickHandler = (): void => {
	const sendCatalogSubscribeBtn = document.getElementById('sendCatalogSubscribeBtn') as HTMLButtonElement
	sendCatalogSubscribeBtn.addEventListener('click', async () => {
		const form = getFormElement()
		const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
		const catalogTrackName = form['catalog-track-name'].value.trim()
		const catalogSubscribeId = BigInt(form['catalog-subscribe-id'].value)
		const catalogTrackAlias = BigInt(form['catalog-track-alias'].value)

		if (!catalogTrackName) {
			setCatalogTrackStatus('Catalog track is required')
			return
		}
		setupCatalogCallbacks(catalogTrackAlias)
		await moqtClient.subscribe(catalogSubscribeId, catalogTrackAlias, trackNamespace, catalogTrackName, AUTH_INFO)
		setCatalogTrackStatus(`Catalog subscribed: ${catalogTrackName}`)
	})
}

const sendSubscribeButtonClickHandler = (): void => {
	const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
	sendSubscribeBtn.addEventListener('click', async () => {
		const form = getFormElement()
		const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
		const selectedVideoTrack = selectedVideoTrackName ?? ''
		const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
		const videoTrackAlias = BigInt(form['video-track-alias'].value)
		const audioSubscribeId = BigInt(form['audio-subscribe-id'].value)
		const audioTrackAlias = BigInt(form['audio-track-alias'].value)

		if (!selectedVideoTrack) {
			setCatalogTrackStatus('Select a video track from catalog first')
			return
		}

		const videoTrack = catalogVideoTracks.find((t) => t.name === selectedVideoTrack)
		const audioTrack = catalogAudioTracks[0]

		if (!videoTrack?.codec || !audioTrack?.codec) {
			setCatalogTrackStatus('Video or audio codec not found in catalog')
			return
		}

		setupMediaSource(videoTrack.codec, audioTrack.codec)
		setupVideoObjectCallbacks(videoTrackAlias)
		setupAudioObjectCallbacks(audioTrackAlias)
		await moqtClient.subscribe(videoSubscribeId, videoTrackAlias, trackNamespace, selectedVideoTrack, AUTH_INFO)
		await moqtClient.subscribe(audioSubscribeId, audioTrackAlias, trackNamespace, audioTrack.name, AUTH_INFO)
		setCatalogTrackStatus(`Subscribed video=${selectedVideoTrack} audio=${audioTrack.name}`)
	})
}

const setupCloseButtonHandler = (): void => {
	const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
	closeBtn.addEventListener('click', async () => {
		await moqtClient.disconnect()
		moqtClient.clearSubgroupObjectHandlers()
		catalogVideoTracks = []
		catalogAudioTracks = []
		selectedVideoTrackName = null
		playbackStarted = false
		videoInitReceived = false
		videoAppendQueue.length = 0
		isVideoAppending = false
		videoSourceBuffer = null
		audioInitReceived = false
		audioAppendQueue.length = 0
		isAudioAppending = false
		audioSourceBuffer = null
		if (mediaSource && mediaSource.readyState === 'open') {
			mediaSource.endOfStream()
		}
		mediaSource = null
		renderCatalogTracks()
		setCatalogTrackStatus('Disconnected')
	})
}

const setupButtonHandlers = (): void => {
	if (handlersInitialized) {
		return
	}
	sendSetupButtonClickHandler()
	sendCatalogSubscribeButtonClickHandler()
	sendSubscribeButtonClickHandler()
	setupCatalogSelectionHandler()
	setupCloseButtonHandler()
	handlersInitialized = true
}

// --- Client callbacks ---

moqtClient.setOnServerSetupHandler((serverSetup: any) => {
	console.log({ serverSetup })
})

moqtClient.setOnSubscribeResponseHandler((subscribeResponse) => {
	console.log({ subscribeResponse })
})

// --- Init ---

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
	const form = getFormElement()
	const url = form.url.value

	await moqtClient.connect(url, { sendSetup: false })
	setCatalogTrackStatus('Connected')
})

setupButtonHandlers()
renderCatalogTracks()
