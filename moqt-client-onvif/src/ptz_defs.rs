pub const DEVICE_NS: &str = "http://www.onvif.org/ver10/device/wsdl";
pub const MEDIA_NS: &str = "http://www.onvif.org/ver10/media/wsdl";
pub const PTZ_NS: &str = "http://www.onvif.org/ver10/ptz/wsdl";
pub const TT_NS: &str = "http://www.onvif.org/ver10/schema";
pub const PAN_TILT_POSITION_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/PositionGenericSpace";
pub const ZOOM_POSITION_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/ZoomSpaces/PositionGenericSpace";
pub const PAN_TILT_SPEED_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/GenericSpeedSpace";
pub const ZOOM_SPEED_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/ZoomSpaces/ZoomGenericSpeedSpace";
pub const PTZ_PAN_MIN: f32 = -1.0;
pub const PTZ_PAN_MAX: f32 = 1.0;
pub const PTZ_PAN_STEP: f32 = 0.1;
pub const PTZ_TILT_MIN: f32 = -1.0;
pub const PTZ_TILT_MAX: f32 = 1.0;
pub const PTZ_TILT_STEP: f32 = 0.1;
pub const DEVICE_NAMESPACES: &str = r#"
  xmlns:tds="http://www.onvif.org/ver10/device/wsdl""#;

pub fn media_namespaces(namespace: &str) -> String {
    format!("\n  xmlns:trt=\"{}\"", namespace)
}
