use crate::catalog::Catalog;
use crate::track::{Dependency, Track};
use crate::types::{KnownPackaging, KnownTrackRole, Packaging, TrackRole};

/// draft-ietf-moq-msf-02 §5.2.2: a namespace yields 1 to 32 fields.
const MAX_NAMESPACE_FIELDS: usize = 32;

/// Why a catalog is invalid under draft-ietf-moq-msf-02, or `Ok` when every
/// track has a non-empty name (§5.2.3) and a well-formed namespace (§5.2.2),
/// every `depends` element resolves (§5.2.15), and every track carries the
/// fields its packaging and role require (§5.2.4.1, §8.3).
pub(crate) fn validate(catalog: &Catalog) -> Result<(), String> {
    let tracks = catalog.tracks.as_deref().unwrap_or_default();
    for track in tracks {
        if track.name.is_empty() {
            return Err("a track has an empty name".to_string());
        }
        if let Some(namespace) = &track.namespace {
            check_namespace(namespace)
                .map_err(|reason| format!("track {}: {reason}", track.name))?;
        }
        for dependency in track.depends.iter().flatten() {
            check_dependency(tracks, track, dependency)
                .map_err(|reason| format!("track {}: {reason}", track.name))?;
        }
        if let Some(field) = missing_required_field(track) {
            return Err(format!("track {} has no {field}", track.name));
        }
    }
    Ok(())
}

fn check_namespace(namespace: &str) -> Result<(), String> {
    let fields: Vec<&str> = namespace.split('/').collect();
    if fields.iter().any(|field| field.is_empty()) {
        return Err(format!("namespace {namespace:?} has an empty field"));
    }
    if fields.len() > MAX_NAMESPACE_FIELDS {
        return Err(format!(
            "namespace {namespace:?} has more than {MAX_NAMESPACE_FIELDS} fields"
        ));
    }
    Ok(())
}

fn check_dependency(
    tracks: &[Track],
    track: &Track,
    dependency: &Dependency,
) -> Result<(), String> {
    match dependency {
        Dependency::TrackName(name) => {
            let matches = tracks
                .iter()
                .filter(|other| !std::ptr::eq(*other, track) && &other.name == name)
                .count();
            if matches != 1 {
                return Err(format!(
                    "depends names {name:?}, which {matches} other tracks have"
                ));
            }
        }
        Dependency::RenderGroup(render_group) => {
            if !tracks
                .iter()
                .any(|other| other.render_group == Some(*render_group))
            {
                return Err(format!(
                    "depends names renderGroup {render_group}, which no track has"
                ));
            }
        }
    }
    Ok(())
}

/// The first field the table of §5.2.4.1, or §8.3 for an event timeline,
/// requires of `track` that it does not carry.
fn missing_required_field(track: &Track) -> Option<&'static str> {
    let Packaging::Known(packaging) = &track.packaging else {
        return None;
    };
    let role = match &track.role {
        Some(TrackRole::Known(role)) => Some(role),
        _ => None,
    };
    let required: &[(&str, bool)] = match (packaging, role) {
        (KnownPackaging::Loc, Some(KnownTrackRole::Audio)) => &[
            ("codec", track.codec.is_some()),
            ("bitrate", track.bitrate.is_some()),
            ("samplerate", track.sample_rate.is_some()),
            ("channelConfig", track.channel_config.is_some()),
        ],
        (KnownPackaging::Loc, Some(KnownTrackRole::Video)) => &[
            ("codec", track.codec.is_some()),
            ("bitrate", track.bitrate.is_some()),
            ("framerate", track.framerate.is_some()),
        ],
        (KnownPackaging::Loc, Some(KnownTrackRole::AudioDescription)) => &[
            ("codec", track.codec.is_some()),
            ("samplerate", track.sample_rate.is_some()),
            ("channelConfig", track.channel_config.is_some()),
            ("lang", track.lang.is_some()),
        ],
        (KnownPackaging::Loc, Some(KnownTrackRole::SignLanguage)) => &[
            ("codec", track.codec.is_some()),
            ("framerate", track.framerate.is_some()),
        ],
        (KnownPackaging::EventTimeline, _) => &[("eventType", track.event_type.is_some())],
        _ => &[],
    };
    required
        .iter()
        .find(|(_, present)| !present)
        .map(|(field, _)| *field)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(tracks: Vec<Track>) -> Catalog {
        Catalog::new(tracks, 0)
    }

    fn loc_video(name: &str) -> Track {
        let mut track = Track::new(name, KnownPackaging::Loc, true);
        track.role = Some(KnownTrackRole::Video.into());
        track.codec = Some("avc1.42e01e".to_string());
        track.bitrate = Some(1_000_000);
        track.framerate = Some(30.0);
        track
    }

    fn with_depends(mut track: Track, depends: Vec<Dependency>) -> Track {
        track.depends = Some(depends);
        track
    }

    #[test]
    fn a_catalog_whose_tracks_meet_the_draft_is_valid() {
        // Arrange
        let mut video = loc_video("video");
        video.namespace = Some("example.com/live/channel1".to_string());
        video.render_group = Some(1);
        let timeline = with_depends(
            Track::new("video_timeline", KnownPackaging::MediaTimeline, true),
            vec![Dependency::RenderGroup(1)],
        );
        let mut chat = with_depends(
            Track::new("chat", KnownPackaging::EventTimeline, true),
            vec!["video".into()],
        );
        chat.event_type = Some("com.example.chat".to_string());

        // Act / Assert
        assert_eq!(validate(&catalog(vec![video, timeline, chat])), Ok(()));
    }

    #[test]
    fn a_track_with_an_empty_name_is_invalid() {
        // Act / Assert
        assert!(validate(&catalog(vec![Track::new("", KnownPackaging::Cmaf, true)])).is_err());
    }

    #[test]
    fn a_namespace_with_an_empty_field_or_over_32_fields_is_invalid() {
        // Arrange
        let namespaces = ["", "/live", "live/", "live//a", &["a"; 33].join("/")];

        // Act / Assert
        for namespace in namespaces {
            let mut track = Track::new("media", KnownPackaging::Cmaf, true);
            track.namespace = Some(namespace.to_string());
            assert!(validate(&catalog(vec![track])).is_err(), "{namespace:?}");
        }
    }

    #[test]
    fn a_dependency_that_does_not_resolve_to_one_other_track_or_a_render_group_is_invalid() {
        // Arrange
        let timeline = |depends| {
            with_depends(
                Track::new("timeline", KnownPackaging::MediaTimeline, true),
                depends,
            )
        };
        let catalogs = [
            catalog(vec![timeline(vec!["video".into()])]),
            catalog(vec![timeline(vec!["timeline".into()])]),
            catalog(vec![
                loc_video("video"),
                loc_video("video"),
                timeline(vec!["video".into()]),
            ]),
            catalog(vec![
                loc_video("video"),
                timeline(vec![Dependency::RenderGroup(1)]),
            ]),
        ];

        // Act / Assert
        for catalog in catalogs {
            assert!(validate(&catalog).is_err(), "{catalog:?}");
        }
    }

    #[test]
    fn a_track_without_a_field_its_packaging_and_role_require_is_invalid() {
        // Arrange
        let mut video = loc_video("video");
        video.bitrate = None;
        let mut audio = Track::new("audio", KnownPackaging::Loc, true);
        audio.role = Some(KnownTrackRole::Audio.into());
        audio.codec = Some("opus".to_string());
        audio.bitrate = Some(64_000);
        audio.sample_rate = Some(48_000);
        let event_timeline = Track::new("chat", KnownPackaging::EventTimeline, true);

        // Act
        let errors = [video, audio, event_timeline].map(|track| validate(&catalog(vec![track])));

        // Assert
        assert_eq!(
            errors,
            [
                Err("track video has no bitrate".to_string()),
                Err("track audio has no channelConfig".to_string()),
                Err("track chat has no eventType".to_string()),
            ]
        );
    }

    #[test]
    fn a_loc_track_of_another_role_requires_nothing_more() {
        // Arrange
        let mut subtitle = Track::new("subtitle", KnownPackaging::Loc, true);
        subtitle.role = Some(KnownTrackRole::Subtitle.into());

        // Act / Assert
        assert_eq!(validate(&catalog(vec![subtitle])), Ok(()));
    }
}
