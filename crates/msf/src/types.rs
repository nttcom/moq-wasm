use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownPackaging {
    Loc,
    Cmaf,
    MediaTimeline,
    EventTimeline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Packaging {
    Known(KnownPackaging),
    Other(String),
}

impl From<KnownPackaging> for Packaging {
    fn from(packaging: KnownPackaging) -> Self {
        Self::Known(packaging)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownTrackRole {
    AudioDescription,
    Video,
    Audio,
    MediaTimeline,
    EventTimeline,
    Caption,
    Subtitle,
    SignLanguage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TrackRole {
    Known(KnownTrackRole),
    Other(String),
}

impl From<KnownTrackRole> for TrackRole {
    fn from(role: KnownTrackRole) -> Self {
        Self::Known(role)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmaf_packaging_serializes_to_the_cmsf_value() {
        // Arrange
        let packaging = Packaging::Known(KnownPackaging::Cmaf);

        // Act
        let json = serde_json::to_string(&packaging).unwrap();

        // Assert
        assert_eq!(json, "\"cmaf\"");
        assert_eq!(serde_json::from_str::<Packaging>(&json).unwrap(), packaging);
    }
}
