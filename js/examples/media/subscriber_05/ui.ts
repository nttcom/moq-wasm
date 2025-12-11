
import { MoqtClientWrapper } from '@moqt/moqtClient';
import { getFormElement } from './utils';
import { connect, setupClientObjectCallbacks, audioDecoderWorker, videoDecoderWorker } from './core';
import { AUTH_INFO } from './const';

function sendSetupButtonClickHandler(client: MoqtClientWrapper) {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement;
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement();

    const versions = new BigUint64Array('0xff00000A'.split(',').map(BigInt));
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value);

    await client.sendSetupMessage(versions, maxSubscribeId);
  });
}

function sendSubscribeButtonClickHandler(client: MoqtClientWrapper) {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement;
  sendSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement();
    const trackNamespace = form['subscribe-track-namespace'].value.split('/');
    setupClientObjectCallbacks(client, 'video', Number(0));
    await client.subscribe(
      BigInt(0),
      BigInt(0),
      trackNamespace,
      'video',
      AUTH_INFO
    );

    setupClientObjectCallbacks(client, 'audio', Number(1));
    await client.subscribe(
      BigInt(1),
      BigInt(1),
      trackNamespace,
      'audio',
      AUTH_INFO
    );
  });
}

function setupAudioDecoderWorker() {
  const audioGenerator = new MediaStreamTrackGenerator({ kind: 'audio' });
  const audioWriter = audioGenerator.writable.getWriter();
  const audioStream = new MediaStream([audioGenerator]);
  audioDecoderWorker.onmessage = async (e: MessageEvent) => {
    const audioData = e.data.audioData;
    await audioWriter.write(audioData);
  };

  const audioContext = new AudioContext();
  const analyser = audioContext.createAnalyser();
  const source = audioContext.createMediaStreamSource(audioStream);
  source.connect(analyser);
  analyser.connect(audioContext.destination);

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

function setupVideoDecoderWorker() {
  const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' });
  const videoWriter = videoGenerator.writable.getWriter();
  const videoStream = new MediaStream([videoGenerator]);
  const videoElement = document.getElementById('video') as HTMLFormElement;
  videoElement.srcObject = videoStream;

  // 遅延表示用のHTML要素を取得
  const latencyElement = document.getElementById('latency');

  videoDecoderWorker.onmessage = async (e: MessageEvent) => {
    const videoFrame = e.data.frame as VideoFrame | undefined;
    const senderTimestamp = e.data.senderTimestamp as number | undefined;

    // 遅延を計算して表示
    if (senderTimestamp && latencyElement) {
      const now = Date.now();
      const latency = now - senderTimestamp;
      latencyElement.innerText = `E2E Latency: ${latency} ms`;
    }

    if (!videoFrame) {
      return;
    }
    await videoWriter.write(videoFrame);
    videoFrame.close();
    await videoElement.play();
  };
}

function setupButtonEventHandlers(client: MoqtClientWrapper) {
  sendSetupButtonClickHandler(client);
  sendSubscribeButtonClickHandler(client);
}

export function setupUI() {
    const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement;
    connectBtn.addEventListener('click', async () => {
        const form = getFormElement();
        const url = form.url.value;
        const client = await connect(url);
        setupButtonEventHandlers(client);
        setupAudioDecoderWorker();
        setupVideoDecoderWorker();
    });
}
