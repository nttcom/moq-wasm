pub const DEVICE_ACTION_NS: &str = "http://www.onvif.org/ver10/device/wsdl";
pub const MEDIA_ACTION_NS: &str = "http://www.onvif.org/ver10/media/wsdl";
pub const PTZ_ACTION_NS: &str = "http://www.onvif.org/ver20/ptz/wsdl";
pub const TT_NS: &str = "http://www.onvif.org/ver10/schema";

pub const PAN_TILT_POSITION_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/PositionGenericSpace";
pub const ZOOM_POSITION_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/ZoomSpaces/PositionGenericSpace";
pub const PAN_TILT_TRANSLATION_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/TranslationGenericSpace";
pub const ZOOM_TRANSLATION_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/ZoomSpaces/TranslationGenericSpace";
pub const PAN_TILT_VELOCITY_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/VelocityGenericSpace";
pub const ZOOM_VELOCITY_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/ZoomSpaces/VelocityGenericSpace";
pub const PAN_TILT_SPEED_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/GenericSpeedSpace";
pub const ZOOM_SPEED_SPACE: &str =
    "http://www.onvif.org/ver10/tptz/ZoomSpaces/ZoomGenericSpeedSpace";

pub struct OnvifRequest {
    pub namespace: &'static str,
    pub operation: &'static str,
    pub body: String,
}

#[derive(Clone, Copy)]
pub struct PtzMoveRequest {
    pub pan: f32,
    pub tilt: f32,
    pub zoom: f32,
    pub speed: f32,
}

#[derive(Clone, Copy)]
pub struct PtzMoveSpaces<'a> {
    pub pan_tilt: Option<&'a str>,
    pub zoom: Option<&'a str>,
    pub pan_tilt_speed: Option<&'a str>,
    pub zoom_speed: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub struct PtzVelocityRequest {
    pub pan: f32,
    pub tilt: f32,
    pub zoom: f32,
}

#[derive(Clone, Copy)]
pub struct PtzVelocitySpaces<'a> {
    pub pan_tilt: Option<&'a str>,
    pub zoom: Option<&'a str>,
}

pub fn get_services() -> OnvifRequest {
    build(
        DEVICE_ACTION_NS,
        "GetServices",
        format!(
            r#"<GetServices xmlns="{DEVICE_ACTION_NS}"><IncludeCapability>false</IncludeCapability></GetServices>"#
        ),
    )
}

pub fn get_capabilities() -> OnvifRequest {
    build(
        DEVICE_ACTION_NS,
        "GetCapabilities",
        format!(
            r#"<GetCapabilities xmlns="{DEVICE_ACTION_NS}"><Category>All</Category></GetCapabilities>"#
        ),
    )
}

pub fn get_profiles() -> OnvifRequest {
    build(
        MEDIA_ACTION_NS,
        "GetProfiles",
        format!(r#"<GetProfiles xmlns="{MEDIA_ACTION_NS}"/>"#),
    )
}

pub fn get_stream_uri(profile_token: &str) -> OnvifRequest {
    build(
        MEDIA_ACTION_NS,
        "GetStreamUri",
        format!(
            r#"<GetStreamUri xmlns="{MEDIA_ACTION_NS}"><StreamSetup><Stream>RTP-Unicast</Stream><Transport><Protocol>RTSP</Protocol></Transport></StreamSetup><ProfileToken>{profile_token}</ProfileToken></GetStreamUri>"#
        ),
    )
}

pub fn get_configurations() -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "GetConfigurations",
        format!(r#"<GetConfigurations xmlns="{PTZ_ACTION_NS}"/>"#),
    )
}

pub fn get_configuration_options(token: &str) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "GetConfigurationOptions",
        format!(
            r#"<GetConfigurationOptions xmlns="{PTZ_ACTION_NS}"><ConfigurationToken>{token}</ConfigurationToken></GetConfigurationOptions>"#
        ),
    )
}

pub fn absolute_move(
    profile: &str,
    movement: PtzMoveRequest,
    spaces: PtzMoveSpaces<'_>,
) -> OnvifRequest {
    let position = position_elements(
        movement,
        spaces,
        PAN_TILT_POSITION_SPACE,
        ZOOM_POSITION_SPACE,
    );
    let speed = speed_elements(movement.speed, spaces);
    build(
        PTZ_ACTION_NS,
        "AbsoluteMove",
        format!(
            r#"<AbsoluteMove xmlns="{PTZ_ACTION_NS}"><ProfileToken>{profile}</ProfileToken><Position>{position}</Position><Speed>{speed}</Speed></AbsoluteMove>"#
        ),
    )
}

pub fn relative_move(
    profile: &str,
    movement: PtzMoveRequest,
    spaces: PtzMoveSpaces<'_>,
) -> OnvifRequest {
    let translation = position_elements(
        movement,
        spaces,
        PAN_TILT_TRANSLATION_SPACE,
        ZOOM_TRANSLATION_SPACE,
    );
    let speed = speed_elements(movement.speed, spaces);
    build(
        PTZ_ACTION_NS,
        "RelativeMove",
        format!(
            r#"<RelativeMove xmlns="{PTZ_ACTION_NS}"><ProfileToken>{profile}</ProfileToken><Translation>{translation}</Translation><Speed>{speed}</Speed></RelativeMove>"#
        ),
    )
}

pub fn continuous_move(
    profile: &str,
    velocity: PtzVelocityRequest,
    spaces: PtzVelocitySpaces<'_>,
) -> OnvifRequest {
    let pan_tilt_space = spaces.pan_tilt.unwrap_or(PAN_TILT_VELOCITY_SPACE);
    let zoom_space = spaces.zoom.unwrap_or(ZOOM_VELOCITY_SPACE);
    let velocity = format!(
        "{}{}",
        pan_tilt_element(velocity.pan, velocity.tilt, pan_tilt_space),
        zoom_element(velocity.zoom, zoom_space)
    );
    build(
        PTZ_ACTION_NS,
        "ContinuousMove",
        format!(
            r#"<ContinuousMove xmlns="{PTZ_ACTION_NS}"><ProfileToken>{profile}</ProfileToken><Velocity>{velocity}</Velocity></ContinuousMove>"#
        ),
    )
}

pub fn stop(profile: &str, pan_tilt: bool, zoom: bool) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "Stop",
        format!(
            r#"<Stop xmlns="{PTZ_ACTION_NS}"><ProfileToken>{profile}</ProfileToken><PanTilt>{pan_tilt}</PanTilt><Zoom>{zoom}</Zoom></Stop>"#
        ),
    )
}

fn build(namespace: &'static str, operation: &'static str, body: String) -> OnvifRequest {
    OnvifRequest {
        namespace,
        operation,
        body,
    }
}

fn position_elements(
    movement: PtzMoveRequest,
    spaces: PtzMoveSpaces<'_>,
    default_pan_tilt_space: &str,
    default_zoom_space: &str,
) -> String {
    let pan_tilt_space = spaces.pan_tilt.unwrap_or(default_pan_tilt_space);
    let zoom_space = spaces.zoom.unwrap_or(default_zoom_space);
    format!(
        "{}{}",
        pan_tilt_element(movement.pan, movement.tilt, pan_tilt_space),
        zoom_element(movement.zoom, zoom_space)
    )
}

fn speed_elements(speed: f32, spaces: PtzMoveSpaces<'_>) -> String {
    let speed = format_float(speed);
    let pan_tilt_space = spaces.pan_tilt_speed.unwrap_or(PAN_TILT_SPEED_SPACE);
    let zoom_space = spaces.zoom_speed.unwrap_or(ZOOM_SPEED_SPACE);
    format!(
        r#"<PanTilt x="{speed}" y="{speed}" xmlns="{TT_NS}" space="{pan_tilt_space}"/><Zoom x="{speed}" xmlns="{TT_NS}" space="{zoom_space}"/>"#
    )
}

fn pan_tilt_element(pan: f32, tilt: f32, space: &str) -> String {
    let pan = format_float(pan);
    let tilt = format_float(tilt);
    format!(r#"<PanTilt x="{pan}" y="{tilt}" xmlns="{TT_NS}" space="{space}"></PanTilt>"#)
}

fn zoom_element(zoom: f32, space: &str) -> String {
    let zoom = format_float(zoom);
    format!(r#"<Zoom x="{zoom}" xmlns="{TT_NS}" space="{space}"></Zoom>"#)
}

fn format_float(value: f32) -> String {
    let normalized = if value.abs() < f32::EPSILON {
        0.0
    } else {
        value
    };
    format!("{:.3}", normalized)
}
