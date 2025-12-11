
import { MOQTClient } from '../../../pkg/moqt_client_sample';
import { getFormElement } from './utils';
import { 
    mediaStream, 
    videoElement, 
    setMediaStream, 
    setVideoElement, 
    startSendingMedia, 
    getClient, 
    connect, 
    resetAudioEncoderWorker
} from './core';
import { AUTH_INFO } from './const';
import { audioContext, gainNode, analyser } from './context';

function setupAudioProcessing(stream: MediaStream): MediaStream {
    const audioTracks = stream.getAudioTracks();
    if (audioTracks.length === 0) {
        return stream; // No audio track, so no processing needed.
    }

    const source = audioContext.createMediaStreamSource(stream);
    source.connect(gainNode);

    const volumeSlider = document.getElementById('volume') as HTMLInputElement;
    gainNode.gain.value = parseFloat(volumeSlider.value);
    volumeSlider.addEventListener('input', () => {
        gainNode.gain.value = parseFloat(volumeSlider.value);
    });

    gainNode.connect(analyser);

    const destination = audioContext.createMediaStreamDestination();
    gainNode.connect(destination);

    // Create a new MediaStream with the original video tracks and the processed audio track.
    const videoTracks = stream.getVideoTracks();
    const processedAudioTracks = destination.stream.getAudioTracks();
    return new MediaStream([...videoTracks, ...processedAudioTracks]);
}

export function setupAudioLevelMeter() {
  analyser.fftSize = 256;
  const bufferLength = analyser.frequencyBinCount;
  const dataArray = new Uint8Array(bufferLength);

  const canvas = document.getElementById('level') as HTMLCanvasElement;
  const canvasCtx = canvas.getContext('2d');

  function draw() {
    requestAnimationFrame(draw);

    analyser.getByteFrequencyData(dataArray);

    if (!canvasCtx) {
      return;
    }
    canvasCtx.fillStyle = 'rgb(200, 200, 200)';
    canvasCtx.fillRect(0, 0, canvas.width, canvas.height);

    const barWidth = (canvas.width / bufferLength) * 2.5;
    let barHeight;
    let x = 0;

    for (let i = 0; i < bufferLength; i++) {
      barHeight = dataArray[i];

      canvasCtx.fillStyle = 'rgb(' + (barHeight + 100) + ',50,50)';
      canvasCtx.fillRect(x, canvas.height - barHeight / 2, barWidth, barHeight / 2);

      x += barWidth + 1;
    }
  }

  draw();
}

export function setUpStartGetUserMediaButton() {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
  startGetUserMediaBtn.addEventListener('click', async () => {
    if (audioContext.state === 'suspended') {
      await audioContext.resume();
    }
    resetAudioEncoderWorker();
    const constraints = {
      audio: true,
      video: {
        width: { exact: 1920 },
        height: { exact: 1080 }
      }
    }
    const stream = await navigator.mediaDevices.getUserMedia(constraints)
    const processedStream = setupAudioProcessing(stream);
    setMediaStream(processedStream);
    const video = document.getElementById('video') as HTMLVideoElement
    video.srcObject = processedStream;

    setupAudioLevelMeter();
  })
}

export function setUpStartGetDisplayMediaButton() {
  const startGetDisplayMediaBtn = document.getElementById('startGetDisplayMediaBtn') as HTMLButtonElement
  startGetDisplayMediaBtn.addEventListener('click', async () => {
    if (audioContext.state === 'suspended') {
      await audioContext.resume();
    }
    resetAudioEncoderWorker();

    const constraints = {
      audio: {
        channelCount: 2,
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false
      },
      video: {
        width: 1920,
        height: 1080
      }
    }
    const displayStream = await navigator.mediaDevices.getDisplayMedia(constraints)

    if (videoElement) {
      videoElement.remove()
    }

    const newVideoElement = document.createElement('video')
    newVideoElement.srcObject = displayStream
    newVideoElement.muted = true
    newVideoElement.play().then(() => {
      // @ts-ignore
      const stream = newVideoElement.captureStream()
      const processedStream = setupAudioProcessing(stream);
      setMediaStream(processedStream);
      console.log('mediaStream (from captureStream):', mediaStream)
      console.log('audio tracks:', mediaStream!.getAudioTracks())
      console.log('video tracks:', mediaStream!.getVideoTracks())

      const video = document.getElementById('video') as HTMLVideoElement
      video.srcObject = mediaStream

      if (mediaStream!.getAudioTracks().length > 0) {
        setupAudioLevelMeter()
      } else {
        console.warn('No audio track found in getDisplayMedia stream.')
      }
    })
    setVideoElement(newVideoElement);
  })
}

function sendSetupButtonClickHandler(client: MOQTClient): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()

    const versions = new BigUint64Array('0xff00000A'.split(',').map(BigInt))
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await client.sendSetupMessage(versions, maxSubscribeId)
  })
}

function sendAnnounceButtonClickHandler(client: MOQTClient): void {
  const sendAnnounceBtn = document.getElementById('sendAnnounceBtn') as HTMLButtonElement
  sendAnnounceBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = form['announce-track-namespace'].value.split('/')

    await client.sendAnnounceMessage(trackNamespace, AUTH_INFO)
  })
}

function sendSubgroupObjectButtonClickHandler(client: MOQTClient): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('clicked sendSubgroupObjectBtn')
    startSendingMedia(client);
  })
}

export function setupButtonEventHandlers(client: MOQTClient): void {
  sendSetupButtonClickHandler(client)
  sendAnnounceButtonClickHandler(client)
  sendSubgroupObjectButtonClickHandler(client)
}


function formatTime(seconds: number): string {
    const minutes = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
}

export function setUpLoadFileButton() {
  const loadFileBtn = document.getElementById('loadFileBtn') as HTMLButtonElement;
  loadFileBtn.addEventListener('click', async () => {
    resetAudioEncoderWorker();

    const fileInput = document.getElementById('fileInput') as HTMLInputElement;
    const file = fileInput.files?.[0];
    if (!file) {
      console.error('No file selected');
      return;
    }

    const videoURL = URL.createObjectURL(file);

    if (videoElement) {
      videoElement.remove();
    }

    const newVideoElement = document.createElement('video');
    newVideoElement.src = videoURL;
    newVideoElement.muted = false;
    newVideoElement.loop = true;
    newVideoElement.controls = true;
    newVideoElement.style.display = 'none';
    document.body.appendChild(newVideoElement);
    setVideoElement(newVideoElement);

    const video = document.getElementById('video') as HTMLVideoElement;
    video.srcObject = null;
    video.src = videoURL;
    video.muted = true;

    const seekBar = document.getElementById('seekBar') as HTMLInputElement;
    const timeDisplay = document.getElementById('timeDisplay') as HTMLParagraphElement;

    newVideoElement.addEventListener('loadedmetadata', () => {
        seekBar.max = newVideoElement.duration.toString();
        timeDisplay.textContent = `${formatTime(newVideoElement.currentTime)} / ${formatTime(newVideoElement.duration)}`;
    });

    newVideoElement.addEventListener('timeupdate', () => {
        seekBar.value = newVideoElement.currentTime.toString();
        timeDisplay.textContent = `${formatTime(newVideoElement.currentTime)} / ${formatTime(newVideoElement.duration)}`;
    });

    seekBar.addEventListener('input', () => {
        newVideoElement.currentTime = parseFloat(seekBar.value);
        video.currentTime = parseFloat(seekBar.value);
    });
  });
}

export function setUpPlayButton() {
  const playBtn = document.getElementById('playBtn') as HTMLButtonElement;
  playBtn.addEventListener('click', async () => {
    if (audioContext.state === 'suspended') {
      await audioContext.resume();
    }
    if (videoElement) {
      videoElement.play().then(() => {
        console.log("videoElement started playing");
        // @ts-ignore
        const stream = videoElement.captureStream();
        const processedStream = setupAudioProcessing(stream);
        setMediaStream(processedStream);
        setupAudioLevelMeter();
        const client = getClient();
        if (client) {
          startSendingMedia(client);
        } else {
          console.error('Client not initialized');
        }
      }).catch(e => console.error("Play failed:", e));
    } else {
      console.error("videoElement is not ready");
    }
  });

  // videoタグを含むタブがアクティブに戻ったときに位置を補正する
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      const video = document.getElementById('video') as HTMLVideoElement;
      if (!video) return;

      // videoElement が再生中なら、「本物」と同期
      if (videoElement) {
        video.currentTime = videoElement.currentTime;
      }
    }
  });

}

export function setUpPauseButton() {
    const pauseBtn = document.getElementById('pauseBtn') as HTMLButtonElement;
    pauseBtn.addEventListener('click', () => {
        if (videoElement) {
            videoElement.pause();
        }
    });
}

export function setUpConnectButton() {
    const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
    connectBtn.addEventListener('click', async () => {
        const form = getFormElement()
        const url = form.url.value
        const client = await connect(url);
        setupButtonEventHandlers(client);
    });
}