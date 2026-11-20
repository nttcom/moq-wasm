import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import { playableVideoTracks } from './playableVideoTracks'

function track(name: string, codec: string, height: number, bitrate?: number): MediaCatalogTrack {
  return { name, label: name, codec, altGroup: 1, width: (height * 16) / 9, height, bitrate }
}

const HEVC_1080 = track('camera', 'hvc1.1.6.L120.B0', 1080, 1_500_000)
const HEVC_360 = track('camera_360p', 'hvc1.1.6.L63.B0', 360, 400_000)
const AVC_1080 = track('camera_1080p_avc', 'avc1.640028', 1080, 2_500_000)
const AVC_360 = track('camera_360p_avc', 'avc1.64001e', 360, 600_000)
const ALL = [HEVC_1080, HEVC_360, AVC_1080, AVC_360]

const decodes =
  (...families: string[]) =>
  async (codec: string) =>
    families.some((family) => codec.startsWith(family))

test('a viewer that decodes both codecs keeps the lower bitrate of each size', async () => {
  // Act
  const playable = await playableVideoTracks(ALL, decodes('hvc1', 'avc1'))

  // Assert
  expect(playable).toEqual([HEVC_1080, HEVC_360])
})

test('a viewer that does not decode H.265 keeps the H.264 renditions', async () => {
  // Act
  const playable = await playableVideoTracks(ALL, decodes('avc1'))

  // Assert
  expect(playable).toEqual([AVC_1080, AVC_360])
})

test('a size of another switching set, or without a bitrate, stays', async () => {
  // Arrange
  const otherSet = { ...AVC_1080, name: 'screen', altGroup: 2 }
  const noBitrate = { ...AVC_360, name: 'camera_360p_raw', bitrate: undefined }

  // Act
  const playable = await playableVideoTracks([HEVC_1080, otherSet, HEVC_360, noBitrate], decodes('hvc1', 'avc1'))

  // Assert
  expect(playable).toEqual([HEVC_1080, otherSet, HEVC_360, noBitrate])
})

test('without any decodable track every track stays', async () => {
  // Act
  const playable = await playableVideoTracks([HEVC_1080, HEVC_360], decodes('avc1'))

  // Assert
  expect(playable).toEqual([HEVC_1080, HEVC_360])
})
