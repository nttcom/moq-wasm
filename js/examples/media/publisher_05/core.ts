
import init, { MOQTClient } from '../../../pkg/moqt_client_sample';
import { AUTH_INFO, KEYFRAME_INTERVAL } from './const';
import { sendVideoObjectMessage, sendAudioObjectMessage } from './sender';
import { getFormElement } from './utils';
import { audioContext } from './context';
import { postMetrics } from '../subscriber_03/metrics'

export let mediaStream: MediaStream | null = null;
export let videoElement: HTMLVideoElement | null = null;
const activeSubscriptions = new Set<bigint>();

export function setMediaStream(stream: MediaStream) {
    mediaStream = stream;
}

export function setVideoElement(element: HTMLVideoElement) {
    videoElement = element;
}

const LatestMediaTrackInfo: {
  video: {
    objectId: bigint
    groupId: bigint
    subgroups: {}
  }
  audio: {
    objectId: bigint
    groupId: bigint
    subgroups: {
      [key: number]: {
        isSendedSubgroupHeader: boolean
      }
    }
  }
} = {
    video: {
      objectId: 0n,
      groupId: -1n,
      isHeaderSentForCurrentGroup: false,
      subgroups: {
        0: {}
        // 1: {}, 
        // 2: {}
      }
    },  audio: {
    objectId: 0n,
    groupId: 0n,
    subgroups: {
      0: { isSendedSubgroupHeader: false }
    }
  }
}

export const videoEncoderWorker = new Worker(new URL('./videoEncoder.ts', import.meta.url), { type: 'module' });
async function handleVideoChunkMessage(
  chunk: EncodedVideoChunk,
  metadata: EncodedVideoChunkMetadata | undefined,
  client: MOQTClient
) {
  const form = getFormElement()
  const trackAlias = BigInt(form['video-object-track-alias'].value)
  const publisherPriority = form['video-publisher-priority'].value

  if (!activeSubscriptions.has(trackAlias)) {
    return; // Drop chunk if no active subscription
  }

  if (chunk.type === 'key') {
    LatestMediaTrackInfo['video'].groupId++
    LatestMediaTrackInfo['video'].objectId = BigInt(0)
    LatestMediaTrackInfo['video'].isHeaderSentForCurrentGroup = false

    const subgroupKeys = Object.keys(LatestMediaTrackInfo['video'].subgroups).map(BigInt)
    for (const subgroup of subgroupKeys) {
      await client.sendSubgroupStreamHeaderMessage(
        BigInt(trackAlias),
        LatestMediaTrackInfo['video'].groupId,
        // @ts-ignore - The SVC property is not defined in the standard but actually exists
        subgroup,
        publisherPriority
      )
      console.log('send subgroup stream header')
    }
    LatestMediaTrackInfo['video'].isHeaderSentForCurrentGroup = true
  }

  if (!LatestMediaTrackInfo['video'].isHeaderSentForCurrentGroup) {
    return;
  }

  sendVideoObjectMessage(
    trackAlias,
    LatestMediaTrackInfo['video'].groupId,
    // @ts-ignore - The SVC property is not defined in the standard but actually exists
    BigInt(metadata?.svc.temporalLayerId), // = subgroupId
    LatestMediaTrackInfo['video'].objectId,
    chunk,
    client
  )
  LatestMediaTrackInfo['video'].objectId++
}

export let audioEncoderWorker = new Worker(new URL('./audioEncoder.ts', import.meta.url), { type: 'module' });

export function resetAudioEncoderWorker() {
    audioEncoderWorker = new Worker(new URL('./audioEncoder_2ch.ts', import.meta.url), { type: 'module' });
}

async function handleAudioChunkMessage(
  chunk: EncodedAudioChunk,
  metadata: EncodedAudioChunkMetadata | undefined,
  client: MOQTClient
) {
  const form = getFormElement()
  const trackAlias = BigInt(form['audio-object-track-alias'].value)
  const publisherPriority = form['audio-publisher-priority'].value

  if (!activeSubscriptions.has(trackAlias)) {
    return;
  }
  const subgroupId = 0

  if (!LatestMediaTrackInfo['audio']['subgroups'][subgroupId].isSendedSubgroupHeader) {
    await client.sendSubgroupStreamHeaderMessage(
      BigInt(trackAlias),
      LatestMediaTrackInfo['audio'].groupId,
      BigInt(subgroupId),
      publisherPriority
    )
    console.log('send subgroup stream header')
    LatestMediaTrackInfo['audio']['subgroups'][subgroupId].isSendedSubgroupHeader = true
  }

  sendAudioObjectMessage(
    trackAlias,
    LatestMediaTrackInfo['audio'].groupId,
    BigInt(subgroupId),
    LatestMediaTrackInfo['audio'].objectId,
    chunk,
    client
  )
  LatestMediaTrackInfo['audio'].objectId++
}

export function setupClientCallbacks(client: MOQTClient): void {
/*
  client.onPacketInfo((packetInfo: any) => {
    postMetrics( 'pub', `packet_object_id`, packetInfo.object_id )
      //console.log({ packetInfo });
  });
*/
  client.onSetup(async (serverSetup: any) => {
    console.log({ serverSetup })
  })
/*
  client.onAnnounce(async (announceMessage: any) => {
    console.log({ announceMessage })
    const announcedNamespace = announceMessage.track_namespace

    await client.sendAnnounceOkMessage(announcedNamespace)
  })

  client.onAnnounceResponce(async (announceResponceMessage: any) => {
    console.log({ announceResponceMessage })
  })
*/
  client.onSubscribe(async (subscribeMessage: any, isSuccess: any, code: any) => {
    console.log(subscribeMessage)
    console.log(subscribeMessage.subscribeId)
    console.log(subscribeMessage.trackAlias)
    const form = getFormElement()
    const receivedSubscribeId = BigInt(subscribeMessage.subscribeId)
    const receivedTrackAlias = BigInt(subscribeMessage.trackAlias)
    console.log('subscribeId', receivedSubscribeId, 'trackAlias', receivedTrackAlias)

    if (isSuccess) {
      activeSubscriptions.add(receivedTrackAlias);
      console.log(`New active subscription for track alias: ${receivedTrackAlias}. Total: ${activeSubscriptions.size}`);
      const expire = 0n
      const forwardingPreference = (Array.from(form['forwarding-preference']) as HTMLInputElement[]).filter(
        (elem) => elem.checked
      )[0].value
      await client.sendSubscribeOkMessage(receivedSubscribeId, expire, AUTH_INFO, forwardingPreference)
    } else {
      const reasonPhrase = 'subscribe error'
      await client.sendSubscribeErrorMessage(subscribeMessage.subscribeId, code, reasonPhrase)
    }
  })
}

export async function startSendingMedia(client: MOQTClient) {
    if (mediaStream == null) {
      console.error('mediaStream is null')
      return
    }
    videoEncoderWorker.onmessage = async (e: MessageEvent) => {
      const { chunk, metadata } = e.data as {
        chunk: EncodedVideoChunk
        metadata: EncodedVideoChunkMetadata | undefined
      }
      handleVideoChunkMessage(chunk, metadata, client)
    }
    audioEncoderWorker.onmessage = async (e: MessageEvent) => { 
      const { chunk, metadata } = e.data as {
        chunk: EncodedAudioChunk
        metadata: EncodedAudioChunkMetadata | undefined
      }
      handleAudioChunkMessage(chunk, metadata, client)
    }

    const videoTracks = mediaStream.getVideoTracks()
    if (videoTracks.length > 0) {
      const [videoTrack] = videoTracks
      const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
      const videoStream = videoProcessor.readable
      videoEncoderWorker.postMessage({
        type: 'keyframeInterval',
        keyframeInterval: KEYFRAME_INTERVAL
      })
      videoEncoderWorker.postMessage({ type: 'videoStream', videoStream: videoStream }, [videoStream])
    }

    const audioTracks = mediaStream.getAudioTracks()
    if (audioTracks.length > 0) {
        console.log('Using AudioWorklet path');
        await audioContext.audioWorklet.addModule('./audio-processor.js');
        const source = audioContext.createMediaStreamSource(mediaStream);
        const workletNode = new AudioWorkletNode(audioContext, 'audio-processor');

        workletNode.port.onmessage = (event) => {
          audioEncoderWorker.postMessage({
            type: 'audioData',
            left: event.data.left,
            right: event.data.right,
            sampleRate: audioContext.sampleRate
          }, [event.data.left.buffer, event.data.right.buffer]);
        };
        source.connect(workletNode);
    }
}

export let client: MOQTClient | null = null;

export async function connect(url: string) {
    client = new MOQTClient(url);
    setupClientCallbacks(client);
    await client.start();
    return client;
}

export function getClient(): MOQTClient | null {
    return client;
}

export async function initialize() {
    await init();
}
