//! The part of the libobs C ABI this plugin uses, declared by hand from the
//! OBS Studio 32.0 headers (`obs.h`, `obs-defs.h`, `obs-output.h`,
//! `obs-service.h`, `obs-encoder.h`).

use std::ffi::{c_char, c_int, c_void};

/// `LIBOBS_API_VER` of the headers these declarations follow. libobs refuses
/// a module that reports a newer major or minor version than its own.
pub(crate) const LIBOBS_API_VER: u32 = 32 << 24;

pub(crate) const OBS_OUTPUT_AV: u32 = (1 << 0) | (1 << 1);
pub(crate) const OBS_OUTPUT_ENCODED: u32 = 1 << 2;
pub(crate) const OBS_OUTPUT_SERVICE: u32 = 1 << 3;

pub(crate) const OBS_OUTPUT_CONNECT_FAILED: c_int = -2;
pub(crate) const OBS_OUTPUT_DISCONNECTED: c_int = -5;

/// `enum obs_encoder_type`.
pub(crate) const OBS_ENCODER_VIDEO: c_int = 1;

macro_rules! opaque {
    ($($name:ident),*) => {$(
        #[repr(C)]
        pub(crate) struct $name {
            _opaque: [u8; 0],
        }
    )*};
}
opaque!(ObsData, ObsEncoder, ObsModule, ObsOutput, ObsService);

/// A callback slot this plugin leaves empty.
type Unused = Option<unsafe extern "C" fn()>;

/// `struct encoder_packet`.
#[repr(C)]
pub(crate) struct EncoderPacket {
    pub(crate) data: *mut u8,
    pub(crate) size: usize,
    pub(crate) pts: i64,
    pub(crate) dts: i64,
    pub(crate) timebase_num: i32,
    pub(crate) timebase_den: i32,
    pub(crate) encoder_type: c_int,
    pub(crate) keyframe: bool,
    pub(crate) dts_usec: i64,
    pub(crate) sys_dts_usec: i64,
    pub(crate) priority: c_int,
    pub(crate) drop_priority: c_int,
    pub(crate) track_idx: usize,
    pub(crate) encoder: *mut ObsEncoder,
}

/// `struct obs_output_info`.
#[repr(C)]
pub(crate) struct ObsOutputInfo {
    pub(crate) id: *const c_char,
    pub(crate) flags: u32,
    pub(crate) get_name: Option<unsafe extern "C" fn(*mut c_void) -> *const c_char>,
    pub(crate) create: Option<unsafe extern "C" fn(*mut ObsData, *mut ObsOutput) -> *mut c_void>,
    pub(crate) destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    pub(crate) start: Option<unsafe extern "C" fn(*mut c_void) -> bool>,
    pub(crate) stop: Option<unsafe extern "C" fn(*mut c_void, u64)>,
    pub(crate) raw_video: Unused,
    pub(crate) raw_audio: Unused,
    pub(crate) encoded_packet: Option<unsafe extern "C" fn(*mut c_void, *mut EncoderPacket)>,
    pub(crate) update: Unused,
    pub(crate) get_defaults: Unused,
    pub(crate) get_properties: Unused,
    pub(crate) unused1: Unused,
    pub(crate) get_total_bytes: Option<unsafe extern "C" fn(*mut c_void) -> u64>,
    pub(crate) get_dropped_frames: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    pub(crate) type_data: *mut c_void,
    pub(crate) free_type_data: Unused,
    pub(crate) get_congestion: Unused,
    pub(crate) get_connect_time_ms: Unused,
    pub(crate) encoded_video_codecs: *const c_char,
    pub(crate) encoded_audio_codecs: *const c_char,
    pub(crate) raw_audio2: Unused,
    pub(crate) protocols: *const c_char,
}

/// `struct obs_service_info`.
#[repr(C)]
pub(crate) struct ObsServiceInfo {
    pub(crate) id: *const c_char,
    pub(crate) get_name: Option<unsafe extern "C" fn(*mut c_void) -> *const c_char>,
    pub(crate) create: Option<unsafe extern "C" fn(*mut ObsData, *mut ObsService) -> *mut c_void>,
    pub(crate) destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    pub(crate) activate: Unused,
    pub(crate) deactivate: Unused,
    pub(crate) update: Unused,
    pub(crate) get_defaults: Unused,
    pub(crate) get_properties: Unused,
    pub(crate) initialize: Unused,
    pub(crate) get_url: Unused,
    pub(crate) get_key: Unused,
    pub(crate) get_username: Unused,
    pub(crate) get_password: Unused,
    pub(crate) deprecated_1: Unused,
    pub(crate) apply_encoder_settings:
        Option<unsafe extern "C" fn(*mut c_void, *mut ObsData, *mut ObsData)>,
    pub(crate) type_data: *mut c_void,
    pub(crate) free_type_data: Unused,
    pub(crate) get_output_type: Unused,
    pub(crate) get_supported_resolutions: Unused,
    pub(crate) get_max_fps: Unused,
    pub(crate) get_max_bitrate: Unused,
    pub(crate) get_supported_video_codecs: Unused,
    pub(crate) get_protocol: Option<unsafe extern "C" fn(*mut c_void) -> *const c_char>,
    pub(crate) get_supported_audio_codecs: Unused,
    pub(crate) get_connect_info: Unused,
    pub(crate) can_try_to_connect: Unused,
}

// Windows links every import at build time and OBS ships no import library,
// so rustc makes one from these declarations.
#[cfg_attr(windows, link(name = "obs", kind = "raw-dylib"))]
unsafe extern "C" {
    pub(crate) fn obs_register_output_s(info: *const ObsOutputInfo, size: usize);
    pub(crate) fn obs_register_service_s(info: *const ObsServiceInfo, size: usize);

    pub(crate) fn obs_output_get_service(output: *const ObsOutput) -> *mut ObsService;
    pub(crate) fn obs_output_get_video_encoder(output: *const ObsOutput) -> *mut ObsEncoder;
    pub(crate) fn obs_output_get_audio_encoder(
        output: *const ObsOutput,
        idx: usize,
    ) -> *mut ObsEncoder;
    pub(crate) fn obs_output_can_begin_data_capture(output: *const ObsOutput, flags: u32) -> bool;
    pub(crate) fn obs_output_initialize_encoders(output: *mut ObsOutput, flags: u32) -> bool;
    pub(crate) fn obs_output_begin_data_capture(output: *mut ObsOutput, flags: u32) -> bool;
    pub(crate) fn obs_output_end_data_capture(output: *mut ObsOutput);
    pub(crate) fn obs_output_signal_stop(output: *mut ObsOutput, code: c_int);
    pub(crate) fn obs_output_set_last_error(output: *mut ObsOutput, message: *const c_char);

    pub(crate) fn obs_encoder_get_extra_data(
        encoder: *const ObsEncoder,
        extra_data: *mut *mut u8,
        size: *mut usize,
    ) -> bool;

    /// Returns a new reference that `obs_data_release` gives back.
    pub(crate) fn obs_service_get_settings(service: *const ObsService) -> *mut ObsData;
    pub(crate) fn obs_data_get_string(data: *mut ObsData, name: *const c_char) -> *const c_char;
    pub(crate) fn obs_data_get_bool(data: *mut ObsData, name: *const c_char) -> bool;
    pub(crate) fn obs_data_get_int(data: *mut ObsData, name: *const c_char) -> i64;
    pub(crate) fn obs_data_set_int(data: *mut ObsData, name: *const c_char, value: i64);
    pub(crate) fn obs_data_release(data: *mut ObsData);
}
