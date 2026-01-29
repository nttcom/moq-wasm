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
            r#"<GetServices xmlns="{}"><IncludeCapability>false</IncludeCapability></GetServices>"#,
            DEVICE_ACTION_NS
        ),
    )
}

pub fn get_capabilities() -> OnvifRequest {
    build(
        DEVICE_ACTION_NS,
        "GetCapabilities",
        format!(
            r#"<GetCapabilities xmlns="{}"><Category>All</Category></GetCapabilities>"#,
            DEVICE_ACTION_NS
        ),
    )
}

pub fn get_profiles() -> OnvifRequest {
    build(
        MEDIA_ACTION_NS,
        "GetProfiles",
        format!(r#"<GetProfiles xmlns="{}"/>"#, MEDIA_ACTION_NS),
    )
}

pub fn get_configurations() -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "GetConfigurations",
        format!(r#"<GetConfigurations xmlns="{}"/>"#, PTZ_ACTION_NS),
    )
}

pub fn get_configuration(token: &str) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "GetConfiguration",
        format!(
            r#"<GetConfiguration xmlns="{0}"><ConfigurationToken>{1}</ConfigurationToken></GetConfiguration>"#,
            PTZ_ACTION_NS, token
        ),
    )
}

pub fn get_configuration_options(token: &str) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "GetConfigurationOptions",
        format!(
            r#"<GetConfigurationOptions xmlns="{0}"><ConfigurationToken>{1}</ConfigurationToken></GetConfigurationOptions>"#,
            PTZ_ACTION_NS, token
        ),
    )
}

pub fn get_nodes() -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "GetNodes",
        format!(r#"<GetNodes xmlns="{}"/>"#, PTZ_ACTION_NS),
    )
}

pub fn get_node(token: &str) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "GetNode",
        format!(
            r#"<GetNode xmlns="{0}"><NodeToken>{1}</NodeToken></GetNode>"#,
            PTZ_ACTION_NS, token
        ),
    )
}

pub fn absolute_move(
    profile: &str,
    movement: PtzMoveRequest,
    spaces: PtzMoveSpaces<'_>,
) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "AbsoluteMove",
        absolute_move_body(profile, movement, spaces),
    )
}

pub fn relative_move(
    profile: &str,
    movement: PtzMoveRequest,
    spaces: PtzMoveSpaces<'_>,
) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "RelativeMove",
        relative_move_body(profile, movement, spaces),
    )
}

pub fn continuous_move(
    profile: &str,
    velocity: PtzVelocityRequest,
    spaces: PtzVelocitySpaces<'_>,
) -> OnvifRequest {
    build(
        PTZ_ACTION_NS,
        "ContinuousMove",
        continuous_move_body(profile, velocity, spaces),
    )
}

pub fn stop(profile: &str, pan_tilt: bool, zoom: bool) -> OnvifRequest {
    build(PTZ_ACTION_NS, "Stop", stop_body(profile, pan_tilt, zoom))
}

fn build(namespace: &'static str, operation: &'static str, body: String) -> OnvifRequest {
    OnvifRequest {
        namespace,
        operation,
        body,
    }
}

fn absolute_move_body(
    profile: &str,
    movement: PtzMoveRequest,
    spaces: PtzMoveSpaces<'_>,
) -> String {
    let pan_tilt_space = space_or_default(spaces.pan_tilt, PAN_TILT_POSITION_SPACE);
    let zoom_space = space_or_default(spaces.zoom, ZOOM_POSITION_SPACE);
    let pan_tilt_speed_space = space_or_default(spaces.pan_tilt_speed, PAN_TILT_SPEED_SPACE);
    let zoom_speed_space = space_or_default(spaces.zoom_speed, ZOOM_SPEED_SPACE);
    let position = format!(
        "{}{}",
        pan_tilt_element(movement.pan, movement.tilt, pan_tilt_space),
        zoom_element(movement.zoom, zoom_space)
    );
    let speed = format!(
        "{}{}",
        pan_tilt_speed_element(movement.speed, pan_tilt_speed_space),
        zoom_speed_element(movement.speed, zoom_speed_space)
    );
    format!(
        r#"<AbsoluteMove xmlns="{ns}"><ProfileToken>{profile}</ProfileToken><Position>{position}</Position><Speed>{speed}</Speed></AbsoluteMove>"#,
        ns = PTZ_ACTION_NS,
        profile = profile,
        position = position,
        speed = speed
    )
}

fn relative_move_body(
    profile: &str,
    movement: PtzMoveRequest,
    spaces: PtzMoveSpaces<'_>,
) -> String {
    let pan_tilt_space = space_or_default(spaces.pan_tilt, PAN_TILT_TRANSLATION_SPACE);
    let zoom_space = space_or_default(spaces.zoom, ZOOM_TRANSLATION_SPACE);
    let pan_tilt_speed_space = space_or_default(spaces.pan_tilt_speed, PAN_TILT_SPEED_SPACE);
    let zoom_speed_space = space_or_default(spaces.zoom_speed, ZOOM_SPEED_SPACE);
    let translation = format!(
        "{}{}",
        pan_tilt_element(movement.pan, movement.tilt, pan_tilt_space),
        zoom_element(movement.zoom, zoom_space)
    );
    let speed = format!(
        "{}{}",
        pan_tilt_speed_element(movement.speed, pan_tilt_speed_space),
        zoom_speed_element(movement.speed, zoom_speed_space)
    );
    format!(
        r#"<RelativeMove xmlns="{ns}"><ProfileToken>{profile}</ProfileToken><Translation>{translation}</Translation><Speed>{speed}</Speed></RelativeMove>"#,
        ns = PTZ_ACTION_NS,
        profile = profile,
        translation = translation,
        speed = speed
    )
}

fn continuous_move_body(
    profile: &str,
    velocity: PtzVelocityRequest,
    spaces: PtzVelocitySpaces<'_>,
) -> String {
    let pan_tilt_space = space_or_default(spaces.pan_tilt, PAN_TILT_VELOCITY_SPACE);
    let zoom_space = space_or_default(spaces.zoom, ZOOM_VELOCITY_SPACE);
    let velocity = format!(
        "{}{}",
        pan_tilt_element(velocity.pan, velocity.tilt, pan_tilt_space),
        zoom_element(velocity.zoom, zoom_space)
    );
    format!(
        r#"<ContinuousMove xmlns="{ns}"><ProfileToken>{profile}</ProfileToken><Velocity>{velocity}</Velocity></ContinuousMove>"#,
        ns = PTZ_ACTION_NS,
        profile = profile,
        velocity = velocity
    )
}

fn stop_body(profile: &str, pan_tilt: bool, zoom: bool) -> String {
    format!(
        r#"<Stop xmlns="{ns}"><ProfileToken>{profile}</ProfileToken><PanTilt>{pan_tilt}</PanTilt><Zoom>{zoom}</Zoom></Stop>"#,
        ns = PTZ_ACTION_NS,
        profile = profile,
        pan_tilt = pan_tilt,
        zoom = zoom
    )
}

fn pan_tilt_element(pan: f32, tilt: f32, space: &str) -> String {
    let pan = format_float(pan);
    let tilt = format_float(tilt);
    format!(
        r#"<PanTilt x="{pan}" y="{tilt}" xmlns="{tt}" space="{space}"></PanTilt>"#,
        pan = pan,
        tilt = tilt,
        tt = TT_NS,
        space = space
    )
}

fn zoom_element(zoom: f32, space: &str) -> String {
    let zoom = format_float(zoom);
    format!(
        r#"<Zoom x="{zoom}" xmlns="{tt}" space="{space}"></Zoom>"#,
        zoom = zoom,
        tt = TT_NS,
        space = space
    )
}

fn pan_tilt_speed_element(speed: f32, space: &str) -> String {
    let speed = format_float(speed);
    format!(
        r#"<PanTilt x="{speed}" y="{speed}" xmlns="{tt}" space="{space}"/>"#,
        speed = speed,
        tt = TT_NS,
        space = space
    )
}

fn zoom_speed_element(speed: f32, space: &str) -> String {
    let speed = format_float(speed);
    format!(
        r#"<Zoom x="{speed}" xmlns="{tt}" space="{space}"/>"#,
        speed = speed,
        tt = TT_NS,
        space = space
    )
}

fn space_or_default<'a>(space: Option<&'a str>, fallback: &'a str) -> &'a str {
    space.unwrap_or(fallback)
}

fn format_float(value: f32) -> String {
    let normalized = if value.abs() < f32::EPSILON {
        0.0
    } else {
        value
    };
    format!("{:.3}", normalized)
}
