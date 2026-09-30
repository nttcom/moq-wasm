export const CONTROLS_MARKUP = `
<div class="viewer-buffering" data-part="buffering" hidden role="status" aria-label="Buffering" data-testid="live-player-buffering"></div>
<div class="viewer-skip">
  <button type="button" class="viewer-skip-button" data-skip-seconds="-5" data-testid="live-player-skip-back-5-button" aria-label="Back 5 seconds" title="5 秒戻る (↓)">&#x21BA;5</button>
  <button type="button" class="viewer-skip-button" data-skip-seconds="-1" data-testid="live-player-skip-back-1-button" aria-label="Back 1 second" title="1 秒戻る (←)">&#x21BA;1</button>
  <button type="button" class="viewer-skip-button viewer-play" data-part="play-pause" data-testid="live-player-play-pause-button" aria-label="Pause" aria-pressed="false" title="停止 / 再開">&#x275A;&#x275A;</button>
  <button type="button" class="viewer-skip-button" data-skip-seconds="1" data-testid="live-player-skip-forward-1-button" aria-label="Forward 1 second" title="1 秒進む (→)">1&#x21BB;</button>
  <button type="button" class="viewer-skip-button" data-skip-seconds="5" data-testid="live-player-skip-forward-5-button" aria-label="Forward 5 seconds" title="5 秒進む (↑)">5&#x21BB;</button>
</div>
<div class="viewer-overlay">
  <div class="viewer-readout">
    <span data-part="seek-start" data-testid="live-player-seek-start">--:--</span>
    <span data-part="seek-elapsed" data-testid="live-player-seek-elapsed">--:-- / --:--</span>
    <output data-part="seek-position" data-testid="live-player-seek-position">LIVE</output>
  </div>
  <div class="viewer-seek">
    <div class="seek-availability">
      <div class="seek-availability-window" data-part="seek-available-window" data-testid="live-player-seek-available-window"></div>
      <div class="seek-review-progress" data-part="seek-review-progress" hidden data-testid="live-player-seek-review-progress"></div>
    </div>
    <input type="range" min="0" max="0" step="any" value="0" disabled aria-valuetext="Live" data-part="seekbar" data-testid="live-player-seekbar" />
  </div>
  <div class="viewer-controls">
    <button type="button" class="viewer-live" data-part="live" data-testid="live-player-live-button" aria-label="Back to live"><span class="viewer-live-dot" aria-hidden="true"></span>LIVE</button>
    <select class="viewer-speed" data-part="speed" disabled aria-label="Playback speed" title="再生速度（CMAF の巻き戻し中のみ）" data-testid="live-player-speed-select">
      <option value="0.5">0.5x</option>
      <option value="1" selected>1x</option>
      <option value="1.25">1.25x</option>
      <option value="1.5">1.5x</option>
      <option value="2">2x</option>
    </select>
    <label class="viewer-volume" title="音量">
      <span aria-hidden="true">&#x1F50A;</span>
      <input type="range" min="0" max="1" step="0.05" value="1" aria-label="Volume" data-part="volume" data-testid="live-player-volume" />
    </label>
    <div class="viewer-quality">
      <button type="button" class="viewer-button" data-part="quality-button" data-testid="live-player-quality-button" aria-label="Quality settings" title="画質と音声">&#x2699;</button>
      <div class="viewer-menu" popover data-part="quality-menu" data-testid="live-player-quality-menu">
        <label>Video track <select data-part="video-track" data-testid="live-player-video-track-select"></select></label>
        <label>Audio track <select data-part="audio-track" data-testid="live-player-audio-track-select"></select></label>
        <label>Packaging
          <select data-part="packaging" data-testid="live-player-packaging-select">
            <option value="loc">LOC (WebCodecs)</option>
            <option value="cmaf">CMAF (MSE)</option>
          </select>
        </label>
        <label>Min buffer (ms) <input type="number" min="0" max="5000" step="50" value="200" data-part="min-buffer" data-testid="live-player-playout-buffer-input" /></label>
        <label>Max buffer (ms) <input type="number" min="0" max="5000" step="50" placeholder="∞" data-part="max-buffer" data-testid="live-player-max-buffer-input" /></label>
        <output data-part="buffer-current" data-testid="live-player-buffer-current">Current: -</output>
        <label>Catch up
          <select data-part="catch-up" data-testid="live-player-catch-up-select">
            <option value="skip">Skip (trim + crossfade)</option>
            <option value="speed-up">Speed up (WSOLA)</option>
            <option value="off">Off</option>
          </select>
        </label>
      </div>
    </div>
    <button type="button" class="viewer-button" data-part="fullscreen" data-testid="live-player-fullscreen-button" aria-label="Fullscreen" title="全画面">&#x26F6;</button>
  </div>
</div>
`
