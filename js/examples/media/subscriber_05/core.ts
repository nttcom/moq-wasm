import { MoqtClientWrapper } from '@moqt/moqtClient';
import { AUTH_INFO } from './const';
import { getFormElement } from './utils';
import { postMetrics } from './metrics'

let client: MoqtClientWrapper | null = null;

const jitterStats: {
  [trackAlias: number]: {
    lastArrivalTime?: number;
    lastSenderTimestamp?: number;
    jitter?: number;
  };
} = {};

const bitrateStats: {
  totalBytesReceived: number;
  lastLogTime: number;
} = {
  totalBytesReceived: 0,
  lastLogTime: Date.now(),
};

export const audioDecoderWorker = new Worker(new URL('./audioDecoder.ts', import.meta.url), { type: 'module' });
export const videoDecoderWorker = new Worker(new URL('./videoDecoder.ts', import.meta.url), { type: 'module' });

const packetLossStats: {
  [trackAlias: string]: {
    lostPackets: number;
    lastObjectId: number;
  };
} = {};

export function getPacketLossStats() {
  return packetLossStats;
}

export function setupClientObjectCallbacks(client: MoqtClientWrapper, type: 'video' | 'audio', trackAlias: number) {
  const handler = async (groupId: bigint, subgroupStreamObject: any) => {
    const receiverTimestamp = Date.now();
    const senderTimestamp = Number(subgroupStreamObject.timestamp_ms);

    if (!jitterStats[trackAlias]) {
      jitterStats[trackAlias] = {};
    }

    const stats = jitterStats[trackAlias];
    const transitTime = receiverTimestamp - senderTimestamp;

    if (
      stats.lastArrivalTime !== undefined &&
      stats.lastSenderTimestamp !== undefined
    ) {
      const lastTransitTime = stats.lastArrivalTime - stats.lastSenderTimestamp;
      const transitTimeDifference = transitTime - lastTransitTime;
      
      let jitter = stats.jitter || 0;
      jitter += (Math.abs(transitTimeDifference) - jitter) / 16.0;
      stats.jitter = jitter;
        postMetrics( 'sub', `transmit_time_track${trackAlias}`,  transitTime.toFixed(2) )
        postMetrics( 'sub', 'jitter_track${trackAlias}',  jitter.toFixed(2) )
        //console.log(
        //`[Track: ${trackAlias}] Transit Time: ${transitTime.toFixed(
        //  2)}ms, Jitter: ${jitter.toFixed(2)}ms`);
    }

    stats.lastArrivalTime = receiverTimestamp;
    stats.lastSenderTimestamp = senderTimestamp;

    const objectPayload = subgroupStreamObject.objectPayload;
    // Make sure objectPayload is actually a Uint8Array before trying to get its buffer
    if (! (objectPayload instanceof Uint8Array)) {
      console.error('objectPayload is not a Uint8Array:', objectPayload);
      return;
    }

    bitrateStats.totalBytesReceived += objectPayload.length;
    const now = Date.now();
    if (now - bitrateStats.lastLogTime >= 1000) {
      const mbps = (bitrateStats.totalBytesReceived * 8) / 1_000_000;
      //console.log(`[Bitrate] ${mbps.toFixed(2)} Mbps`);
      postMetrics( 'sub', 'bitrate',  mbps.toFixed(2) * 1000 )
      bitrateStats.totalBytesReceived = 0;
      bitrateStats.lastLogTime = now;
    }

    // Create a copy of subgroupStreamObject and replace objectPayload with its ArrayBuffer for transfer
    const subgroupStreamObjectForWorker = {
      ...subgroupStreamObject,
      objectPayload: objectPayload.buffer // Transfer the buffer
    };

    if (type === 'video') {
    if (
      objectPayload.length === 0 ||
      subgroupStreamObject.objectStatus === 'EndOfGroup' ||
      subgroupStreamObject.objectStatus === 'EndOfTrackAndGroup' ||
      subgroupStreamObject.objectStatus === 'EndOfTrack'
    ) {
      return;
    }

    videoDecoderWorker.postMessage({
      groupId,
      subgroupStreamObject: subgroupStreamObjectForWorker,
      senderTimestamp: senderTimestamp,
    }, [objectPayload.buffer]);
  } else {
    audioDecoderWorker.postMessage({
      groupId,
      subgroupStreamObject: subgroupStreamObjectForWorker
    }, [objectPayload.buffer]);
  }
  };
  client.setOnSubgroupObjectHandler(BigInt(trackAlias), handler)
}

export async function connect(url: string): Promise<MoqtClientWrapper> {
  client = new MoqtClientWrapper();
  client.setOnServerSetupHandler(async (serverSetup: any) => {
    console.log({ serverSetup });
  });
  await client.connect(url, { sendSetup: false });
  return client;
}

export function getClient(): MoqtClientWrapper | null {
    return client;
}
