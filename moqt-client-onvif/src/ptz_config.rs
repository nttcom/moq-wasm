use roxmltree::{Document, Node};

#[derive(Clone, Copy, Debug)]
pub struct AxisRange {
    pub min: f32,
    pub max: f32,
}

impl AxisRange {
    pub fn new(min: f32, max: f32) -> Self {
        let (min, max) = ordered_pair(min, max);
        Self { min, max }
    }

    pub fn clamp(self, value: f32) -> f32 {
        value.clamp(self.min, self.max)
    }
}

#[derive(Clone, Debug)]
pub struct PanTiltRange {
    pub uri: Option<String>,
    pub x: AxisRange,
    pub y: AxisRange,
}

#[derive(Clone, Debug)]
pub struct ZoomRange {
    pub uri: Option<String>,
    pub x: AxisRange,
}

#[derive(Clone, Debug)]
pub struct TimeoutRange {
    pub min: String,
    pub max: String,
}

#[derive(Clone, Debug)]
pub struct PtzRange {
    pub abs_pan_tilt: Option<PanTiltRange>,
    pub abs_zoom: Option<ZoomRange>,
    pub rel_pan_tilt: Option<PanTiltRange>,
    pub rel_zoom: Option<ZoomRange>,
    pub cont_pan_tilt: Option<PanTiltRange>,
    pub cont_zoom: Option<ZoomRange>,
    pub speed_pan_tilt: Option<PanTiltRange>,
    pub speed_zoom: Option<ZoomRange>,
    pub timeout: Option<TimeoutRange>,
    pub speed_default: f32,
}

impl Default for PtzRange {
    fn default() -> Self {
        Self {
            abs_pan_tilt: None,
            abs_zoom: None,
            rel_pan_tilt: None,
            rel_zoom: None,
            cont_pan_tilt: None,
            cont_zoom: None,
            speed_pan_tilt: None,
            speed_zoom: None,
            timeout: None,
            speed_default: 1.0,
        }
    }
}

impl PtzRange {
    pub fn absolute_pan_tilt_range(&self) -> (AxisRange, AxisRange) {
        pan_tilt_or_default(self.abs_pan_tilt.as_ref(), None)
    }

    pub fn absolute_zoom_range(&self) -> AxisRange {
        zoom_or_default(self.abs_zoom.as_ref(), None)
    }

    pub fn absolute_pan_tilt_space(&self) -> Option<&str> {
        pan_tilt_space(self.abs_pan_tilt.as_ref(), None)
    }

    pub fn absolute_zoom_space(&self) -> Option<&str> {
        zoom_space(self.abs_zoom.as_ref(), None)
    }

    pub fn relative_pan_tilt_range(&self) -> (AxisRange, AxisRange) {
        pan_tilt_or_default(self.rel_pan_tilt.as_ref(), self.abs_pan_tilt.as_ref())
    }

    pub fn relative_zoom_range(&self) -> AxisRange {
        zoom_or_default(self.rel_zoom.as_ref(), self.abs_zoom.as_ref())
    }

    pub fn relative_pan_tilt_space(&self) -> Option<&str> {
        pan_tilt_space(self.rel_pan_tilt.as_ref(), self.abs_pan_tilt.as_ref())
    }

    pub fn relative_zoom_space(&self) -> Option<&str> {
        zoom_space(self.rel_zoom.as_ref(), self.abs_zoom.as_ref())
    }

    pub fn continuous_pan_tilt_range(&self) -> (AxisRange, AxisRange) {
        pan_tilt_or_default(self.cont_pan_tilt.as_ref(), self.abs_pan_tilt.as_ref())
    }

    pub fn continuous_zoom_range(&self) -> AxisRange {
        zoom_or_default(self.cont_zoom.as_ref(), self.abs_zoom.as_ref())
    }

    pub fn continuous_pan_tilt_space(&self) -> Option<&str> {
        pan_tilt_space(self.cont_pan_tilt.as_ref(), self.abs_pan_tilt.as_ref())
    }

    pub fn continuous_zoom_space(&self) -> Option<&str> {
        zoom_space(self.cont_zoom.as_ref(), self.abs_zoom.as_ref())
    }

    pub fn speed_range(&self) -> AxisRange {
        if let Some(range) = &self.speed_pan_tilt {
            let min = range.x.min.min(range.y.min);
            let max = range.x.max.max(range.y.max);
            return AxisRange::new(min, max);
        }
        if let Some(range) = &self.speed_zoom {
            return range.x;
        }
        AxisRange::new(0.0, 1.0)
    }

    pub fn speed_pan_tilt_space(&self) -> Option<&str> {
        pan_tilt_space(self.speed_pan_tilt.as_ref(), None)
    }

    pub fn speed_zoom_space(&self) -> Option<&str> {
        zoom_space(self.speed_zoom.as_ref(), None)
    }

    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let (pan, tilt) = self.absolute_pan_tilt_range();
        let abs_suffix = uri_suffix(self.abs_pan_tilt.as_ref().and_then(|r| r.uri.as_ref()));
        lines.push(format!(
            "Absolute pan/tilt: x=[{:.3}..{:.3}] y=[{:.3}..{:.3}]{}",
            pan.min, pan.max, tilt.min, tilt.max, abs_suffix
        ));
        if let Some(range) = &self.abs_zoom {
            lines.push(format!("Absolute zoom: {}", format_zoom_range(range)));
        }
        if let Some(range) = &self.rel_pan_tilt {
            lines.push(format!(
                "Relative pan/tilt: {}",
                format_pan_tilt_range(range)
            ));
        }
        if let Some(range) = &self.rel_zoom {
            lines.push(format!("Relative zoom: {}", format_zoom_range(range)));
        }
        if let Some(range) = &self.cont_pan_tilt {
            lines.push(format!(
                "Continuous pan/tilt: {}",
                format_pan_tilt_range(range)
            ));
        }
        if let Some(range) = &self.cont_zoom {
            lines.push(format!("Continuous zoom: {}", format_zoom_range(range)));
        }
        if let Some(range) = &self.speed_pan_tilt {
            lines.push(format!("Speed pan/tilt: {}", format_pan_tilt_range(range)));
        }
        if let Some(range) = &self.speed_zoom {
            lines.push(format!("Speed zoom: {}", format_zoom_range(range)));
        }
        let speed = self.speed_range();
        lines.push(format!(
            "Speed range: [{:.3}..{:.3}] default={:.3}",
            speed.min, speed.max, self.speed_default
        ));
        if let Some(timeout) = &self.timeout {
            lines.push(format!("Timeout range: {}..{}", timeout.min, timeout.max));
        }
        lines
    }
}

pub fn extract_tokens(body: &str) -> Vec<String> {
    let Ok(doc) = Document::parse(body) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "PTZConfiguration")
        .filter_map(|node| node.attribute("token").map(str::to_string))
        .collect()
}

pub fn extract_range_from_config(body: &str, token: &str) -> PtzRange {
    let Ok(doc) = Document::parse(body) else {
        return PtzRange::default();
    };
    let Some(config) = doc.descendants().find(|node| {
        node.is_element()
            && node.tag_name().name() == "PTZConfiguration"
            && node.attribute("token") == Some(token)
    }) else {
        return PtzRange::default();
    };
    let node_token = find_text(config, "NodeToken").unwrap_or_else(|| "-".to_string());
    log::info!("  token={token} node_token={node_token}");

    let mut spaces = Vec::new();
    push_kv(
        &mut spaces,
        "abs_pan_tilt",
        find_text(config, "DefaultAbsolutePantTiltPositionSpace"),
    );
    push_kv(
        &mut spaces,
        "rel_pan_tilt",
        find_text(config, "DefaultRelativePanTiltTranslationSpace"),
    );
    push_kv(
        &mut spaces,
        "cont_pan_tilt",
        find_text(config, "DefaultContinuousPanTiltVelocitySpace"),
    );
    if !spaces.is_empty() {
        log::info!("  spaces: {}", spaces.join(", "));
    }

    let mut range = PtzRange::default();
    if let Some((uri, x, y)) = pan_tilt_limits(config) {
        log::info!(
            "  pan/tilt limits: space={} x=[{:.3}..{:.3}] y=[{:.3}..{:.3}]",
            uri.as_deref().unwrap_or("-"),
            x.min,
            x.max,
            y.min,
            y.max
        );
        range.abs_pan_tilt = Some(PanTiltRange { uri, x, y });
    }
    if let Some((space, x, y)) = default_pan_tilt_speed(config) {
        log::info!(
            "  default speed: pan_tilt x={:.3} y={:.3} space={}",
            x,
            y,
            space.as_deref().unwrap_or("-")
        );
        range.speed_default = x.min(y).clamp(0.0, 1.0);
    }
    if let Some(timeout) = find_text(config, "DefaultPTZTimeout") {
        log::info!("  timeout: {timeout}");
    }
    range
}

pub fn update_range_from_options(mut range: PtzRange, body: &str) -> PtzRange {
    let Ok(doc) = Document::parse(body) else {
        return range;
    };
    let Some(options) = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "PTZConfigurationOptions")
    else {
        return range;
    };
    if let Some(spaces) = options
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Spaces")
    {
        for space in spaces.children().filter(|node| node.is_element()) {
            match space.tag_name().name() {
                "AbsolutePanTiltPositionSpace" => {
                    if let Some(parsed) = parse_pan_tilt_space(space) {
                        range.abs_pan_tilt = Some(parsed);
                    }
                }
                "AbsoluteZoomPositionSpace" => {
                    if let Some(parsed) = parse_zoom_space(space) {
                        range.abs_zoom = Some(parsed);
                    }
                }
                "RelativePanTiltTranslationSpace" => {
                    if let Some(parsed) = parse_pan_tilt_space(space) {
                        range.rel_pan_tilt = Some(parsed);
                    }
                }
                "RelativeZoomTranslationSpace" => {
                    if let Some(parsed) = parse_zoom_space(space) {
                        range.rel_zoom = Some(parsed);
                    }
                }
                "ContinuousPanTiltVelocitySpace" => {
                    if let Some(parsed) = parse_pan_tilt_space(space) {
                        range.cont_pan_tilt = Some(parsed);
                    }
                }
                "ContinuousZoomVelocitySpace" => {
                    if let Some(parsed) = parse_zoom_space(space) {
                        range.cont_zoom = Some(parsed);
                    }
                }
                "PanTiltSpeedSpace" => {
                    if let Some(parsed) = parse_pan_tilt_space(space) {
                        range.speed_pan_tilt = Some(parsed);
                    }
                }
                "ZoomSpeedSpace" => {
                    if let Some(parsed) = parse_zoom_space(space) {
                        range.speed_zoom = Some(parsed);
                    }
                }
                _ => {}
            }
        }
    }
    range.timeout = parse_timeout_range(options);
    let speed = range.speed_range();
    range.speed_default = range.speed_default.clamp(speed.min, speed.max);
    range
}

fn format_pan_tilt_range(range: &PanTiltRange) -> String {
    let suffix = uri_suffix(range.uri.as_ref());
    format!(
        "x=[{:.3}..{:.3}] y=[{:.3}..{:.3}]{}",
        range.x.min, range.x.max, range.y.min, range.y.max, suffix
    )
}

fn format_zoom_range(range: &ZoomRange) -> String {
    let suffix = uri_suffix(range.uri.as_ref());
    format!("x=[{:.3}..{:.3}]{}", range.x.min, range.x.max, suffix)
}

fn uri_suffix(uri: Option<&String>) -> String {
    uri.map(|value| format!(" uri={value}")).unwrap_or_default()
}

fn pan_tilt_or_default(
    primary: Option<&PanTiltRange>,
    fallback: Option<&PanTiltRange>,
) -> (AxisRange, AxisRange) {
    let range = primary.or(fallback);
    let fallback_axis = AxisRange::new(-1.0, 1.0);
    let x = range.map(|r| r.x).unwrap_or(fallback_axis);
    let y = range.map(|r| r.y).unwrap_or(fallback_axis);
    (x, y)
}

fn zoom_or_default(primary: Option<&ZoomRange>, fallback: Option<&ZoomRange>) -> AxisRange {
    primary
        .or(fallback)
        .map(|r| r.x)
        .unwrap_or_else(|| AxisRange::new(0.0, 1.0))
}

fn pan_tilt_space<'a>(
    primary: Option<&'a PanTiltRange>,
    fallback: Option<&'a PanTiltRange>,
) -> Option<&'a str> {
    primary
        .and_then(|range| range.uri.as_deref())
        .or_else(|| fallback.and_then(|range| range.uri.as_deref()))
}

fn zoom_space<'a>(
    primary: Option<&'a ZoomRange>,
    fallback: Option<&'a ZoomRange>,
) -> Option<&'a str> {
    primary
        .and_then(|range| range.uri.as_deref())
        .or_else(|| fallback.and_then(|range| range.uri.as_deref()))
}

fn push_kv(items: &mut Vec<String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        items.push(format!("{key}={value}"));
    }
}

fn pan_tilt_limits(config: Node) -> Option<(Option<String>, AxisRange, AxisRange)> {
    let limits = config
        .descendants()
        .find(|node| has_tag(*node, "PanTiltLimits"))?;
    let range = limits.descendants().find(|node| has_tag(*node, "Range"))?;
    let uri = find_text(range, "URI");
    let x_range = range.descendants().find(|node| has_tag(*node, "XRange"))?;
    let y_range = range.descendants().find(|node| has_tag(*node, "YRange"))?;
    let x = parse_axis_range(x_range)?;
    let y = parse_axis_range(y_range)?;
    Some((uri, x, y))
}

fn default_pan_tilt_speed(config: Node) -> Option<(Option<String>, f32, f32)> {
    let speed = config
        .descendants()
        .find(|node| has_tag(*node, "DefaultPTZSpeed"))?;
    let pan_tilt = speed.descendants().find(|node| has_tag(*node, "PanTilt"))?;
    let space = pan_tilt.attribute("space").map(str::to_string);
    let x = parse_f32(pan_tilt.attribute("x")?)?;
    let y = parse_f32(pan_tilt.attribute("y")?)?;
    Some((space, x, y))
}

fn parse_pan_tilt_space(space: Node) -> Option<PanTiltRange> {
    let uri = find_text(space, "URI");
    let x_range = space.descendants().find(|node| has_tag(*node, "XRange"))?;
    let y_range = space.descendants().find(|node| has_tag(*node, "YRange"))?;
    let x = parse_axis_range(x_range)?;
    let y = parse_axis_range(y_range)?;
    Some(PanTiltRange { uri, x, y })
}

fn parse_zoom_space(space: Node) -> Option<ZoomRange> {
    let uri = find_text(space, "URI");
    let x_range = space.descendants().find(|node| has_tag(*node, "XRange"))?;
    let x = parse_axis_range(x_range)?;
    Some(ZoomRange { uri, x })
}

fn parse_timeout_range(options: Node) -> Option<TimeoutRange> {
    let timeout = options
        .descendants()
        .find(|node| has_tag(*node, "PTZTimeout"))?;
    let min = find_text(timeout, "Min")?;
    let max = find_text(timeout, "Max")?;
    Some(TimeoutRange { min, max })
}

fn parse_axis_range(range: Node) -> Option<AxisRange> {
    let min = parse_f32(find_text(range, "Min")?.as_str())?;
    let max = parse_f32(find_text(range, "Max")?.as_str())?;
    Some(AxisRange::new(min, max))
}

fn ordered_pair(a: f32, b: f32) -> (f32, f32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn parse_f32(value: &str) -> Option<f32> {
    value.trim().parse::<f32>().ok()
}

fn find_text(node: Node, tag: &str) -> Option<String> {
    node.descendants()
        .find(|child| has_tag(*child, tag))
        .and_then(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn has_tag(node: Node, tag: &str) -> bool {
    node.is_element() && node.tag_name().name() == tag
}
