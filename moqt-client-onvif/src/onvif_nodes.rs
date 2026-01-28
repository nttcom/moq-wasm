use crate::{onvif_client::OnvifClient, onvif_command, soap};
use anyhow::Result;
use roxmltree::Document;

#[derive(Clone, Debug)]
pub struct PtzSupportedSpace {
    pub name: String,
    pub uri: String,
}

#[derive(Clone, Debug)]
pub struct PtzNodeInfo {
    pub token: String,
    pub home_supported: Option<bool>,
    pub max_presets: Option<u32>,
    pub spaces: Vec<PtzSupportedSpace>,
}

impl PtzNodeInfo {
    pub fn supports_uri(&self, uri: &str) -> bool {
        if self.spaces.is_empty() {
            return true;
        }
        self.spaces.iter().any(|space| space.uri == uri)
    }

    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("Node token: {}", self.token),
            format!("Home supported: {:?}", self.home_supported),
        ];
        if let Some(max) = self.max_presets {
            lines.push(format!("Max presets: {max}"));
        }
        if self.spaces.is_empty() {
            lines.push("Supported spaces: (not reported)".to_string());
            return lines;
        }
        lines.push("Supported spaces:".to_string());
        for space in &self.spaces {
            lines.push(format!("  {}={}", space.name, space.uri));
        }
        lines
    }
}
pub struct NodesFetch {
    pub response: soap::SoapResponse,
    pub tokens: Vec<String>,
}

pub struct NodeFetch {
    pub response: soap::SoapResponse,
    pub info: Option<PtzNodeInfo>,
}

pub async fn fetch_nodes(onvif: &OnvifClient) -> Result<NodesFetch> {
    let cmd = onvif_command::get_nodes();
    let response = onvif.send_ptz(&cmd).await?;
    let tokens = parse_node_tokens(&response.body);
    Ok(NodesFetch { response, tokens })
}

pub async fn fetch_node(onvif: &OnvifClient, token: &str) -> Result<NodeFetch> {
    let cmd = onvif_command::get_node(token);
    let response = onvif.send_ptz(&cmd).await?;
    let info = parse_node(&response.body);
    Ok(NodeFetch { response, info })
}

pub fn log_nodes(endpoint: &str, nodes: &NodesFetch) {
    log::info!("[GetNodes]");
    soap::log_response("GetNodes", endpoint, &nodes.response);
    if nodes.response.status < 400 && !nodes.tokens.is_empty() {
        log::info!("  nodes: {}", nodes.tokens.join(", "));
    }
}

pub fn log_node(endpoint: &str, node: &NodeFetch) {
    log::info!("[GetNode]");
    soap::log_response("GetNode", endpoint, &node.response);
    let Some(node_info) = node.info.as_ref() else {
        return;
    };
    log::info!(
        "node: token={} home_supported={:?} max_presets={:?}",
        node_info.token,
        node_info.home_supported,
        node_info.max_presets
    );
    let spaces = format_spaces(&node_info.spaces);
    if !spaces.is_empty() {
        log::info!("  spaces: {spaces}");
    }
}

fn parse_node_tokens(body: &str) -> Vec<String> {
    let Ok(doc) = Document::parse(body) else {
        return Vec::new();
    };
    let mut tokens: Vec<String> = doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "PTZNode")
        .filter_map(|node| node.attribute("token").map(str::to_string))
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn parse_node(body: &str) -> Option<PtzNodeInfo> {
    let doc = Document::parse(body).ok()?;
    let node = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "PTZNode")?;
    let token = node.attribute("token")?.to_string();
    let home = node
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "HomeSupported")
        .and_then(|n| n.text())
        .and_then(parse_bool);
    let max = node
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "MaximumNumberOfPresets")
        .and_then(|n| n.text())
        .and_then(|t| t.trim().parse::<u32>().ok());
    let spaces = node
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "SupportedPTZSpaces")
        .map(parse_spaces)
        .unwrap_or_default();
    Some(PtzNodeInfo {
        token,
        home_supported: home,
        max_presets: max,
        spaces,
    })
}

fn parse_spaces(spaces: roxmltree::Node) -> Vec<PtzSupportedSpace> {
    spaces
        .children()
        .filter(|node| node.is_element())
        .filter_map(|space| {
            let name = space.tag_name().name();
            let uri = space
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "URI")
                .and_then(|node| node.text())
                .map(str::trim)
                .filter(|text| !text.is_empty())?;
            Some(PtzSupportedSpace {
                name: name.to_string(),
                uri: uri.to_string(),
            })
        })
        .collect()
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.trim() {
        "true" | "True" | "TRUE" | "1" => Some(true),
        "false" | "False" | "FALSE" | "0" => Some(false),
        _ => None,
    }
}

fn format_spaces(spaces: &[PtzSupportedSpace]) -> String {
    spaces
        .iter()
        .map(|space| format!("{}={}", space.name, space.uri))
        .collect::<Vec<_>>()
        .join(", ")
}
