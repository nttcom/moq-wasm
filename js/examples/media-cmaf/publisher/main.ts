import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO, CMAF_FPS } from '../const'
import { getFormElement } from '../utils'
import { MediaTransportState } from '../../../utils/media/transportState'
import { MEDIA_VIDEO_PROFILES, MEDIA_AUDIO_PROFILES, MEDIA_CATALOG_TRACK_NAME, buildCmafCatalogJson } from '../catalog'
import { Output, NullTarget, Mp4OutputFormat, MediaStreamVideoTrackSource, MediaStreamAudioTrackSource } from 'mediabunny'

let mediaStream: MediaStream | null = null
const moqtClient = new MoqtClientWrapper()
const transportState = new MediaTransportState()
const catalogAliases = new Set<string>()

const ensureClient = (): MOQTClient => {
	const client = moqtClient.getRawClient()
	if (!client) {
		throw new Error('MOQT client not connected')
	}
	return client
}

const setUpStartGetUserMediaButton = () => {
	const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
	startGetUserMediaBtn.addEventListener('click', async () => {
		const constraints = {
			audio: true,
			video: {
				width: { exact: 1280 },
				height: { exact: 720 }
			}
		}
		mediaStream = await navigator.mediaDevices.getUserMedia(constraints)
		const video = document.getElementById('video') as HTMLVideoElement
		video.srcObject = mediaStream
	})
}

const sendMoqObject = async (
	trackAlias: bigint,
	groupId: bigint,
	objectId: bigint,
	data: Uint8Array,
	client: MOQTClient
): Promise<void> => {
	await client.sendSubgroupStreamObject(trackAlias, groupId, 0n, objectId, undefined, data, undefined)
}

const parseTrackNamespace = (raw: string): string[] => {
	return raw
		.split('/')
		.map((part) => part.trim())
		.filter((part) => part.length > 0)
}

const isCatalogTrack = (trackName: string): boolean => {
	return trackName === MEDIA_CATALOG_TRACK_NAME
}

const isVideoTrack = (trackName: string): boolean => {
	return MEDIA_VIDEO_PROFILES.some((profile) => profile.trackName === trackName)
}

const isAudioTrack = (trackName: string): boolean => {
	return MEDIA_AUDIO_PROFILES.some((profile) => profile.trackName === trackName)
}

const getVideoTrackAliases = (client: MOQTClient, trackNamespace: string[]): bigint[] => {
	const aliases = new Set<string>()
	for (const profile of MEDIA_VIDEO_PROFILES) {
		for (const alias of client.getTrackSubscribers(trackNamespace, profile.trackName)) {
			aliases.add(alias.toString())
		}
	}
	return Array.from(aliases, (alias) => BigInt(alias))
}

const getAudioTrackAliases = (client: MOQTClient, trackNamespace: string[]): bigint[] => {
	const aliases = new Set<string>()
	for (const profile of MEDIA_AUDIO_PROFILES) {
		for (const alias of client.getTrackSubscribers(trackNamespace, profile.trackName)) {
			aliases.add(alias.toString())
		}
	}
	return Array.from(aliases, (alias) => BigInt(alias))
}

const sendCatalog = async (client: MOQTClient, trackAlias: bigint, trackNamespace: string[]): Promise<void> => {
	const aliasKey = trackAlias.toString()
	if (catalogAliases.has(aliasKey)) {
		return
	}
	const payload = new TextEncoder().encode(buildCmafCatalogJson(trackNamespace))
	await client.sendSubgroupStreamHeaderMessage(trackAlias, 0n, 0n, 0)
	await client.sendSubgroupStreamObject(trackAlias, 0n, 0n, 0n, undefined, payload, undefined)
	catalogAliases.add(aliasKey)
	console.info('[CmafPublisher] sent catalog', { trackAlias: aliasKey, trackNamespace })
}

const sendSetupButtonClickHandler = (): void => {
	const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
	sendSetupBtn.addEventListener('click', async () => {
		const form = getFormElement()
		const versions = new BigUint64Array('0xff00000A'.split(',').map(BigInt))
		const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
		await moqtClient.sendSetupMessage(versions, maxSubscribeId)
	})
}

const sendAnnounceButtonClickHandler = (): void => {
	const sendAnnounceBtn = document.getElementById('sendAnnounceBtn') as HTMLButtonElement
	sendAnnounceBtn.addEventListener('click', async () => {
		const form = getFormElement()
		const trackNamespace = parseTrackNamespace(form['announce-track-namespace'].value)
		await moqtClient.announce(trackNamespace, AUTH_INFO)
	})
}

const sendSubgroupObjectButtonClickHandler = (): void => {
	const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
	sendSubgroupObjectBtn.addEventListener('click', async () => {
		if (mediaStream == null) {
			console.error('mediaStream is null')
			return
		}
		let client: MOQTClient
		try {
			client = ensureClient()
		} catch (error) {
			console.error(error)
			return
		}

		const form = getFormElement()
		const keyFrameIntervalSec = Number(form['keyframe-interval'].value)

		const getTrackNamespace = () => {
			const form = getFormElement()
			return parseTrackNamespace(form['announce-track-namespace'].value)
		}

		// --- fMP4 fragment コールバック生成 ---
		// mediabunny はキーフレーム境界でフラグメントを切るため、
		// 各フラグメント = 1 MoQ Group（Object 0 = init, Object 1 = moof+mdat）
		const buildMp4Callbacks = (
			label: string,
			getAliases: () => bigint[],
			advanceGroup: () => void,
			getGroupId: () => bigint,
		) => {
			let initSegment: Uint8Array | null = null
			const initParts: Uint8Array[] = []
			let lastMoof: Uint8Array | null = null

			return {
				onFtyp: (data: ArrayBuffer) => {
					initParts.push(new Uint8Array(data))
				},
				onMoov: (data: ArrayBuffer) => {
					initParts.push(new Uint8Array(data))
					const totalLen = initParts.reduce((sum, part) => sum + part.byteLength, 0)
					initSegment = new Uint8Array(totalLen)
					let offset = 0
					for (const part of initParts) {
						initSegment.set(part, offset)
						offset += part.byteLength
					}
					console.info(`[CmafPublisher] ${label} init segment ready`, { byteLength: initSegment.byteLength })
				},
				onMoof: (data: ArrayBuffer) => {
					lastMoof = new Uint8Array(data)
				},
				onMdat: async (data: ArrayBuffer) => {
					if (!lastMoof || !initSegment) {
						return
					}

					const fragment = new Uint8Array(lastMoof.byteLength + data.byteLength)
					fragment.set(lastMoof, 0)
					fragment.set(new Uint8Array(data), lastMoof.byteLength)

					const form = getFormElement()
					const publisherPriority = Number(form['video-publisher-priority'].value)
					const trackAliases = getAliases()
					if (!trackAliases.length) {
						return
					}

					advanceGroup()
					for (const alias of trackAliases) {
						await client.sendSubgroupStreamHeaderMessage(alias, getGroupId(), 0n, publisherPriority)
						await sendMoqObject(alias, getGroupId(), 0n, initSegment, client)
						await sendMoqObject(alias, getGroupId(), 1n, fragment, client)
					}
				},
			}
		}

		// --- 映像用 Output ---
		const videoOutput = new Output({
			target: new NullTarget(),
			format: new Mp4OutputFormat({
				fastStart: 'fragmented',
				...buildMp4Callbacks(
					'video',
					() => getVideoTrackAliases(client, getTrackNamespace()),
					() => transportState.advanceVideoGroup(),
					() => transportState.getVideoGroupId(),
				),
			}),
		})

		// --- 音声用 Output ---
		const audioOutput = new Output({
			target: new NullTarget(),
			format: new Mp4OutputFormat({
				fastStart: 'fragmented',
				...buildMp4Callbacks(
					'audio',
					() => getAudioTrackAliases(client, getTrackNamespace()),
					() => transportState.advanceAudioGroup(),
					() => transportState.getAudioGroupId(),
				),
			}),
		})

		// --- メディアソース (gUM → mediabunny) ---
		const [videoTrack] = mediaStream.getVideoTracks()
		const videoSource = new MediaStreamVideoTrackSource(videoTrack, {
			codec: 'avc',
			bitrate: 2_000_000,
			latencyMode: 'realtime',
			keyFrameInterval: keyFrameIntervalSec,
		})
		videoOutput.addVideoTrack(videoSource, { frameRate: CMAF_FPS })
		videoSource.errorPromise.catch((error) => {
			console.error('[CmafPublisher] video source error', error)
		})

		const [audioTrack] = mediaStream.getAudioTracks()
		if (audioTrack) {
			const audioSource = new MediaStreamAudioTrackSource(audioTrack, {
				codec: 'aac',
				bitrate: 128_000,
			})
			audioOutput.addAudioTrack(audioSource)
			audioSource.errorPromise.catch((error) => {
				console.error('[CmafPublisher] audio source error', error)
			})
		}

		await Promise.all([videoOutput.start(), audioOutput.start()])
		console.info('[CmafPublisher] video/audio outputs started')
	})
}

const setupButtonClickHandler = (): void => {
	sendSetupButtonClickHandler()
	sendAnnounceButtonClickHandler()
	sendSubgroupObjectButtonClickHandler()
}

const setupClientCallbacks = (): void => {
	moqtClient.setOnServerSetupHandler((serverSetup: any) => {
		console.log({ serverSetup })
	})

	moqtClient.setOnAnnounceHandler(async (announceMessage) => {
		console.log({ announceMessage })
		const client = ensureClient()
		await client.sendAnnounceOkMessage(announceMessage.trackNamespace)
	})

	moqtClient.setOnSubscribeResponseHandler((announceResponseMessage) => {
		console.log({ announceResponseMessage })
	})

	moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
		console.log({ subscribeMessage: subscribe })
		const form = getFormElement()
		const trackNamespace = parseTrackNamespace(form['announce-track-namespace'].value)
		const requestedNamespace = subscribe.trackNamespace ?? []
		const trackName = subscribe.trackName ?? ''
		const namespaceMatched = requestedNamespace.join('/') === trackNamespace.join('/')

		if (isSuccess) {
			if (!namespaceMatched) {
				await respondError(404n, 'unknown namespace')
				return
			}
			if (isCatalogTrack(trackName)) {
				await respondOk(0n, AUTH_INFO, 'subgroup')
				const client = ensureClient()
				await sendCatalog(client, BigInt(subscribe.trackAlias), trackNamespace)
				return
			}
			if (isVideoTrack(trackName)) {
				await respondOk(0n, AUTH_INFO, 'subgroup')
				return
			}
			if (isAudioTrack(trackName)) {
				await respondOk(0n, AUTH_INFO, 'subgroup')
				return
			}
			await respondError(404n, 'unknown track')
			return
		}

		const reasonPhrase = `subscribe error: code=${code}`
		await respondError(BigInt(code), reasonPhrase)
	})
}

const setupCloseButtonHandler = (): void => {
	const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
	closeBtn.addEventListener('click', async () => {
		await moqtClient.disconnect()
		catalogAliases.clear()
	})
}

setUpStartGetUserMediaButton()
setupClientCallbacks()
setupCloseButtonHandler()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
	const form = getFormElement()
	const url = form.url.value

	await moqtClient.connect(url, { sendSetup: false })
	catalogAliases.clear()
	setupButtonClickHandler()
})
