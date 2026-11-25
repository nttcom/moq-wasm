use std::{
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
};

use crate::{
    libobs::{
        ObsData, ObsService, ObsServiceInfo, obs_data_get_int, obs_data_set_int,
        obs_register_service_s,
    },
    output::PROTOCOL,
};

const DEFAULT_KEYFRAME_INTERVAL_SECONDS: i64 = 2;

/// Registers `moqt_service`. It holds no state of its own: the output reads
/// the service's settings (`server`, `namespace`, `bearer_token`,
/// `insecure_skip_tls_verify`) when a stream starts.
pub(crate) fn register() {
    let info = ObsServiceInfo {
        id: c"moqt_service".as_ptr(),
        get_name: Some(get_name),
        create: Some(create),
        destroy: Some(destroy),
        activate: None,
        deactivate: None,
        update: None,
        get_defaults: None,
        get_properties: None,
        initialize: None,
        get_url: None,
        get_key: None,
        get_username: None,
        get_password: None,
        deprecated_1: None,
        apply_encoder_settings: Some(apply_encoder_settings),
        type_data: ptr::null_mut(),
        free_type_data: None,
        get_output_type: None,
        get_supported_resolutions: None,
        get_max_fps: None,
        get_max_bitrate: None,
        get_supported_video_codecs: None,
        get_protocol: Some(get_protocol),
        get_supported_audio_codecs: None,
        get_connect_info: None,
        can_try_to_connect: None,
    };
    // SAFETY: libobs copies `info`, and every string in it is static.
    unsafe { obs_register_service_s(&info, size_of::<ObsServiceInfo>()) };
}

unsafe extern "C" fn get_name(_type_data: *mut c_void) -> *const c_char {
    c"MoQT".as_ptr()
}

/// libobs takes a null return as a failure, so a stateless service returns a
/// dangling pointer that is never dereferenced.
unsafe extern "C" fn create(_settings: *mut ObsData, _service: *mut ObsService) -> *mut c_void {
    NonNull::<c_void>::dangling().as_ptr()
}

unsafe extern "C" fn destroy(_data: *mut c_void) {}

/// A viewer starts at a keyframe, and OBS's simple output mode offers no
/// keyframe interval setting, which leaves x264 at 250 frames.
unsafe extern "C" fn apply_encoder_settings(
    _data: *mut c_void,
    video_encoder_settings: *mut ObsData,
    _audio_encoder_settings: *mut ObsData,
) {
    // SAFETY: libobs passes live encoder settings, or null for an encoder it
    // does not have.
    unsafe {
        if !video_encoder_settings.is_null()
            && obs_data_get_int(video_encoder_settings, c"keyint_sec".as_ptr()) == 0
        {
            obs_data_set_int(
                video_encoder_settings,
                c"keyint_sec".as_ptr(),
                DEFAULT_KEYFRAME_INTERVAL_SECONDS,
            );
        }
    }
}

unsafe extern "C" fn get_protocol(_data: *mut c_void) -> *const c_char {
    PROTOCOL.as_ptr()
}
