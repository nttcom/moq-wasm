use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use broadcast::{
    Broadcast, BroadcastWriter, PushOutcome, RelayConnection, SourceTracks, TokenRefreshTask,
};
use mediapack::MediaEvent;
use tokio::{runtime::Runtime, task::JoinHandle, time::timeout};

use crate::{
    libobs::{
        EncoderPacket, OBS_ENCODER_VIDEO, OBS_OUTPUT_AV, OBS_OUTPUT_CONNECT_FAILED,
        OBS_OUTPUT_DISCONNECTED, OBS_OUTPUT_ENCODED, OBS_OUTPUT_SERVICE, ObsData, ObsEncoder,
        ObsOutput, ObsOutputInfo, obs_data_get_bool, obs_data_get_string, obs_data_release,
        obs_encoder_get_extra_data, obs_output_begin_data_capture,
        obs_output_can_begin_data_capture, obs_output_end_data_capture,
        obs_output_get_audio_encoder, obs_output_get_service, obs_output_get_video_encoder,
        obs_output_initialize_encoders, obs_output_set_last_error, obs_output_signal_stop,
        obs_register_output_s, obs_service_get_settings,
    },
    media_input::{MediaInput, ticks_to_micros},
    publish_settings::{PublishSettings, read_token_file},
};

/// The output protocol name that ties `moqt_service` to this output.
pub(crate) const PROTOCOL: &CStr = c"MoQT";

/// Stopping waits this long for the relay to take the session close, so a
/// relay that went away cannot hold up OBS.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn register() {
    let info = ObsOutputInfo {
        id: c"moqt_output".as_ptr(),
        flags: OBS_OUTPUT_AV | OBS_OUTPUT_ENCODED | OBS_OUTPUT_SERVICE,
        get_name: Some(get_name),
        create: Some(create),
        destroy: Some(destroy),
        start: Some(start),
        stop: Some(stop),
        raw_video: None,
        raw_audio: None,
        encoded_packet: Some(encoded_packet),
        update: None,
        get_defaults: None,
        get_properties: None,
        unused1: None,
        get_total_bytes: Some(get_total_bytes),
        get_dropped_frames: Some(get_dropped_frames),
        type_data: ptr::null_mut(),
        free_type_data: None,
        get_congestion: None,
        get_connect_time_ms: None,
        encoded_video_codecs: c"h264".as_ptr(),
        encoded_audio_codecs: c"aac".as_ptr(),
        raw_audio2: None,
        protocols: PROTOCOL.as_ptr(),
    };
    // SAFETY: libobs copies `info`, and every string in it is static.
    unsafe { obs_register_output_s(&info, size_of::<ObsOutputInfo>()) };
}

struct OutputHandle(*mut ObsOutput);

// SAFETY: libobs lets any thread call the output functions used here, as its
// own outputs do from their connect and send threads, and the output outlives
// the `MoqtOutput` that holds the handle.
unsafe impl Send for OutputHandle {}
unsafe impl Sync for OutputHandle {}

struct Publishing {
    writer: BroadcastWriter,
    media: MediaInput,
    /// Re-reads the token file while the session is up.
    _token_refresh: Option<TokenRefreshTask>,
}

impl Publishing {
    async fn open(settings: PublishSettings) -> Result<Self> {
        let connection = RelayConnection::new(Some(settings.target));
        let broadcast = Broadcast::new(connection.clone(), settings.namespace);
        broadcast.announce().await?;
        let token_refresh = settings.token_file.map(|path| {
            TokenRefreshTask::run(connection, move |_| {
                let path = path.clone();
                async move { read_token_file(&path).await }
            })
        });
        Ok(Self {
            writer: BroadcastWriter::run(broadcast, SourceTracks::default()),
            media: MediaInput::default(),
            _token_refresh: token_refresh,
        })
    }
}

struct Shared {
    output: OutputHandle,
    /// Set while the relay session is up. Never held across a libobs call
    /// that can wait for the encoder threads that deliver packets.
    publishing: Mutex<Option<Publishing>>,
    total_bytes: AtomicU64,
    dropped_frames: AtomicI32,
}

struct MoqtOutput {
    shared: Arc<Shared>,
    runtime: Runtime,
    connect_task: Mutex<Option<JoinHandle<()>>>,
}

impl MoqtOutput {
    fn start(&self) -> bool {
        let output = self.shared.output.0;
        let settings = match self.read_settings() {
            Ok(settings) => settings,
            Err(error) => {
                self.shared.set_last_error(&format!("{error:#}"));
                return false;
            }
        };
        // SAFETY: `output` is the live output this instance was created for.
        let ready = unsafe {
            obs_output_can_begin_data_capture(output, 0)
                && obs_output_initialize_encoders(output, 0)
        };
        if !ready {
            return false;
        }
        self.shared.total_bytes.store(0, Ordering::Relaxed);
        self.shared.dropped_frames.store(0, Ordering::Relaxed);
        let task = self.runtime.spawn(self.shared.clone().connect(settings));
        *self.connect_task.lock().expect("connect task lock") = Some(task);
        true
    }

    /// The service's settings, with the token read from the token file when one is set.
    fn read_settings(&self) -> Result<PublishSettings> {
        let settings = self.shared.read_settings()?;
        let Some(path) = &settings.token_file else {
            return Ok(settings);
        };
        let token = self.runtime.block_on(read_token_file(path))?;
        settings.with_token(token)
    }

    fn stop(&self) {
        let publishing = self.cancel();
        // SAFETY: `output` is the live output this instance was created for.
        unsafe { obs_output_end_data_capture(self.shared.output.0) };
        self.close(publishing);
    }

    /// Ends the connect task and takes the session it may have opened. The
    /// task has no await point between storing the session and starting the
    /// data capture, so once it is joined it has done both or neither.
    fn cancel(&self) -> Option<Publishing> {
        if let Some(task) = self.connect_task.lock().expect("connect task lock").take() {
            task.abort();
            let _ = self.runtime.block_on(task);
        }
        self.shared.take_publishing()
    }

    fn close(&self, publishing: Option<Publishing>) {
        let Some(publishing) = publishing else {
            return;
        };
        let broadcast = publishing.writer.broadcast();
        // The timer needs the runtime, so it is made inside `block_on`.
        self.runtime.block_on(async {
            let _ = timeout(CLOSE_TIMEOUT, broadcast.connection().close()).await;
        });
        let track_namespace = broadcast.namespace().join("/");
        tracing::info!(%track_namespace, "OBS output stopped");
    }
}

impl Shared {
    fn read_settings(&self) -> Result<PublishSettings> {
        // SAFETY: the service and its settings are live libobs objects, the
        // strings are copied before the settings are released, and the
        // settings reference taken here is given back.
        unsafe {
            let settings = obs_service_get_settings(obs_output_get_service(self.output.0));
            let result = PublishSettings::new(
                &string_setting(settings, c"server"),
                &string_setting(settings, c"namespace"),
                &string_setting(settings, c"bearer_token"),
                &string_setting(settings, c"bearer_token_file"),
                obs_data_get_bool(settings, c"insecure_skip_tls_verify".as_ptr()),
            );
            obs_data_release(settings);
            result
        }
    }

    async fn connect(self: Arc<Self>, settings: PublishSettings) {
        let url = settings.target.url.clone();
        let track_namespace = settings.namespace.join("/");
        let publishing = match Publishing::open(settings).await {
            Ok(publishing) => publishing,
            Err(error) => {
                let error = format!("{error:#}");
                tracing::error!(%error, %url, %track_namespace, "OBS output failed to connect");
                self.signal_stop(OBS_OUTPUT_CONNECT_FAILED, &error);
                return;
            }
        };
        tracing::info!(%url, %track_namespace, "OBS output connected");
        *self.publishing.lock().expect("publishing lock") = Some(publishing);
        // SAFETY: `output` is live until `destroy`, which joins this task first.
        if !unsafe { obs_output_begin_data_capture(self.output.0, 0) } {
            self.take_publishing();
        }
    }

    fn take_publishing(&self) -> Option<Publishing> {
        self.publishing.lock().expect("publishing lock").take()
    }

    fn encoded_packet(&self, packet: &EncoderPacket) {
        let mut publishing = self.publishing.lock().expect("publishing lock");
        let Some(state) = publishing.as_mut() else {
            return;
        };
        match self.push(state, packet) {
            Ok(()) => {
                self.total_bytes
                    .fetch_add(packet.size as u64, Ordering::Relaxed);
            }
            Err(error) => {
                let track_namespace = state.writer.broadcast().namespace().join("/");
                *publishing = None;
                drop(publishing);
                let error = format!("{error:#}");
                tracing::error!(%error, %track_namespace, "OBS output stopped on an error");
                self.signal_stop(OBS_OUTPUT_DISCONNECTED, &error);
            }
        }
    }

    fn push(&self, state: &mut Publishing, packet: &EncoderPacket) -> Result<()> {
        let is_video = packet.encoder_type == OBS_ENCODER_VIDEO;
        let micros = |ticks| {
            ticks_to_micros(ticks, packet.timebase_num, packet.timebase_den)
                .context("the encoder packet has no usable timebase")
        };
        // SAFETY: libobs keeps the packet data and the encoders' extra data
        // alive for the duration of the `encoded_packet` callback.
        let events = unsafe {
            let payload = slice::from_raw_parts(packet.data, packet.size);
            if is_video {
                let extra_data = extra_data(obs_output_get_video_encoder(self.output.0));
                state.media.video(
                    extra_data,
                    payload,
                    micros(packet.pts)?,
                    micros(packet.dts)?,
                )?
            } else {
                let extra_data = extra_data(obs_output_get_audio_encoder(self.output.0, 0));
                state
                    .media
                    .audio(extra_data, payload, micros(packet.pts)?)?
            }
        };
        for event in events {
            let is_video_sample = matches!(event, MediaEvent::Video(_));
            if state.writer.push(event)? == PushOutcome::Dropped && is_video_sample {
                self.dropped_frames.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn set_last_error(&self, message: &str) {
        let message = CString::new(message.replace('\0', " ")).unwrap_or_default();
        // SAFETY: `output` is live and libobs copies the message.
        unsafe { obs_output_set_last_error(self.output.0, message.as_ptr()) };
    }

    fn signal_stop(&self, code: c_int, message: &str) {
        self.set_last_error(message);
        // SAFETY: `output` is live; libobs ends the capture on its own thread.
        unsafe { obs_output_signal_stop(self.output.0, code) };
    }
}

/// # Safety
/// `encoder` is null or a live libobs encoder, and the returned slice is not
/// used after the encoder is reconfigured or released.
unsafe fn extra_data<'a>(encoder: *const ObsEncoder) -> &'a [u8] {
    let mut data = ptr::null_mut();
    let mut size = 0;
    // SAFETY: the caller's contract; libobs fills both out-parameters on success.
    unsafe {
        if encoder.is_null()
            || !obs_encoder_get_extra_data(encoder, &mut data, &mut size)
            || data.is_null()
        {
            return &[];
        }
        slice::from_raw_parts(data, size)
    }
}

/// # Safety
/// `settings` is a live `obs_data_t`.
unsafe fn string_setting(settings: *mut ObsData, name: &CStr) -> String {
    // SAFETY: the caller's contract; libobs returns a NUL-terminated string
    // (empty when the setting is unset) that is copied here.
    unsafe {
        let value = obs_data_get_string(settings, name.as_ptr());
        if value.is_null() {
            return String::new();
        }
        CStr::from_ptr(value).to_string_lossy().trim().to_owned()
    }
}

unsafe extern "C" fn get_name(_type_data: *mut c_void) -> *const c_char {
    c"MoQT Output".as_ptr()
}

unsafe extern "C" fn create(_settings: *mut ObsData, output: *mut ObsOutput) -> *mut c_void {
    let Ok(runtime) = Runtime::new() else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(MoqtOutput {
        shared: Arc::new(Shared {
            output: OutputHandle(output),
            publishing: Mutex::new(None),
            total_bytes: AtomicU64::new(0),
            dropped_frames: AtomicI32::new(0),
        }),
        runtime,
        connect_task: Mutex::new(None),
    }))
    .cast()
}

/// A panic that left a callback would abort OBS, along with any recording it
/// is making, so it ends here and the callback reports `fallback`.
fn contain_panic<T>(fallback: T, callback: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(callback)).unwrap_or_else(|_| {
        tracing::error!("OBS output callback panicked");
        fallback
    })
}

// SAFETY (all callbacks below): libobs passes the pointer `create` returned,
// and calls `destroy` once, after every other callback has returned.

unsafe extern "C" fn destroy(data: *mut c_void) {
    let output = unsafe { Box::from_raw(data.cast::<MoqtOutput>()) };
    contain_panic((), || {
        let publishing = output.cancel();
        output.close(publishing);
    });
}

unsafe extern "C" fn start(data: *mut c_void) -> bool {
    let output = unsafe { &*data.cast::<MoqtOutput>() };
    contain_panic(false, || output.start())
}

unsafe extern "C" fn stop(data: *mut c_void, _ts: u64) {
    let output = unsafe { &*data.cast::<MoqtOutput>() };
    contain_panic((), || output.stop());
}

unsafe extern "C" fn encoded_packet(data: *mut c_void, packet: *mut EncoderPacket) {
    let output = unsafe { &*data.cast::<MoqtOutput>() };
    let packet = unsafe { &*packet };
    contain_panic((), || output.shared.encoded_packet(packet));
}

unsafe extern "C" fn get_total_bytes(data: *mut c_void) -> u64 {
    let output = unsafe { &*data.cast::<MoqtOutput>() };
    output.shared.total_bytes.load(Ordering::Relaxed)
}

unsafe extern "C" fn get_dropped_frames(data: *mut c_void) -> c_int {
    let output = unsafe { &*data.cast::<MoqtOutput>() };
    output.shared.dropped_frames.load(Ordering::Relaxed)
}
