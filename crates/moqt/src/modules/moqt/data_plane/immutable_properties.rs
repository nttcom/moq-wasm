use crate::modules::moqt::{
    data_plane::{malformed_track_error::MalformedTrackError, object::canonical_object::Object},
    version::Version,
};

/// Fails when `object`, received on a `version` connection, holds Immutable
/// Properties that hold Immutable Properties or a Key-Value-Pair that does
/// not parse: its track is malformed (draft-14 §11.2, draft-18 §12.7).
pub(crate) fn check(version: Version, object: &Object) -> Result<(), MalformedTrackError> {
    if version.has_malformed_immutable_properties(&object.extension_headers) {
        return Err(MalformedTrackError::MalformedImmutableProperties {
            location: object.location,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        modules::{
            moqt::version::Version,
            test_support::{
                PublishedTrack, data_receiver, fetch_answered_with, next_subgroup,
                object_with_extensions, published_track, published_track_over,
                send_subgroup_object, within,
            },
        },
        object::{
            ExtensionHeaders, ForwardingPreference, KeyValuePair, MalformedTrackError, Object,
            StreamReceiveError, Subgroup, VariantType,
        },
    };

    /// Immutable Properties holding `contents`, then a mutable property, in
    /// the ascending type order draft-18 sends them in.
    fn with_immutable(contents: &'static [u8]) -> ExtensionHeaders {
        let mut headers = ExtensionHeaders::default();
        headers.push_immutable_extension(Bytes::from_static(contents));
        headers.key_value_pairs.push(KeyValuePair {
            key: 0x10,
            value: VariantType::Even(1),
        });
        headers
    }

    /// Type 0xB, length 0: Immutable Properties inside Immutable Properties.
    const NESTED: &[u8] = &[0x0B, 0x00];
    /// Odd type 0xD announcing 5 bytes that are not there.
    const UNPARSABLE: &[u8] = &[0x0D, 0x05];
    /// Type 0x2 with value 5, then odd type 0xD with the bytes `ab`; on
    /// draft-18 the second type is a delta of 0xB from the first.
    const WELL_FORMED_DRAFT14: &[u8] = &[0x02, 0x05, 0x0D, 0x02, b'a', b'b'];
    const WELL_FORMED_DRAFT18: &[u8] = &[0x02, 0x05, 0x0B, 0x02, b'a', b'b'];

    #[test]
    fn immutable_properties_that_hold_immutable_properties_are_malformed() {
        for version in [Version::Draft14, Version::Draft18] {
            // Act / Assert: draft-14 §11.2, draft-18 §12.7
            assert!(
                version.has_malformed_immutable_properties(&with_immutable(NESTED)),
                "{version:?}"
            );
        }
    }

    #[test]
    fn immutable_properties_whose_key_value_pair_does_not_parse_are_malformed() {
        for version in [Version::Draft14, Version::Draft18] {
            // Act / Assert: draft-14 §11.2, draft-18 §12.7
            assert!(
                version.has_malformed_immutable_properties(&with_immutable(UNPARSABLE)),
                "{version:?}"
            );
        }
    }

    #[test]
    fn well_formed_immutable_properties_pass() {
        for (version, contents) in [
            (Version::Draft14, WELL_FORMED_DRAFT14),
            (Version::Draft18, WELL_FORMED_DRAFT18),
        ] {
            // Act / Assert
            assert!(
                !version.has_malformed_immutable_properties(&with_immutable(contents)),
                "{version:?}"
            );
            assert!(
                !version.has_malformed_immutable_properties(
                    &object_with_extensions().extension_headers
                ),
                "{version:?}"
            );
        }
    }

    fn nested_object(forwarding: ForwardingPreference) -> Object {
        Object {
            forwarding,
            extension_headers: with_immutable(NESTED),
            ..object_with_extensions()
        }
    }

    fn is_malformed_immutable(error: &MalformedTrackError, object: &Object) -> bool {
        *error
            == MalformedTrackError::MalformedImmutableProperties {
                location: object.location,
            }
    }

    async fn assert_subgroup_object_is_malformed(track: PublishedTrack) {
        // Arrange
        let object = nested_object(object_with_extensions().forwarding);
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        within(send_subgroup_object(
            &track.client,
            &track.published,
            &object,
        ))
        .await;
        let mut stream = within(next_subgroup(&mut receiver)).await;
        within(stream.receive()).await.unwrap();

        // Act
        let received = within(stream.receive()).await;

        // Assert: draft-14 §11.2, draft-18 §12.7
        assert!(
            matches!(&received, Err(StreamReceiveError::MalformedTrack(error))
                if is_malformed_immutable(error, &object)),
            "{received:?}"
        );
    }

    #[tokio::test]
    async fn a_subgroup_object_with_malformed_immutable_properties_is_a_malformed_track() {
        assert_subgroup_object_is_malformed(published_track("immutable-subgroup").await).await;
    }

    #[tokio::test]
    async fn a_draft18_subgroup_object_with_malformed_immutable_properties_is_a_malformed_track() {
        assert_subgroup_object_is_malformed(
            published_track_over("immutable-subgroup-d18", Version::Draft18).await,
        )
        .await;
    }

    async fn assert_datagram_is_malformed(track: PublishedTrack) {
        // Arrange
        let object = nested_object(ForwardingPreference::Datagram);
        let mut sender = track.client.publisher().create_datagram(&track.published);
        let mut receiver = data_receiver(&track.server, &track.accepted).await;

        // Act
        sender.send_object(&object).await.unwrap();
        let received = within(receiver.next()).await;

        // Assert: draft-14 §11.2, draft-18 §12.7
        let error = received.unwrap_err();
        assert!(
            error
                .downcast_ref::<MalformedTrackError>()
                .is_some_and(|error| is_malformed_immutable(error, &object)),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn a_datagram_with_malformed_immutable_properties_is_a_malformed_track() {
        assert_datagram_is_malformed(published_track("immutable-datagram").await).await;
    }

    #[tokio::test]
    async fn a_draft18_datagram_with_malformed_immutable_properties_is_a_malformed_track() {
        assert_datagram_is_malformed(
            published_track_over("immutable-datagram-d18", Version::Draft18).await,
        )
        .await;
    }

    async fn assert_fetch_object_is_malformed(track: PublishedTrack) {
        // Arrange
        let object = nested_object(object_with_extensions().forwarding);

        // Act
        let received = fetch_answered_with(&track, &object).await;

        // Assert: draft-14 §11.2, draft-18 §12.7
        let error = received.unwrap_err();
        assert!(
            error
                .downcast_ref::<MalformedTrackError>()
                .is_some_and(|error| is_malformed_immutable(error, &object)),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn a_fetched_object_with_malformed_immutable_properties_is_a_malformed_track() {
        assert_fetch_object_is_malformed(published_track("immutable-fetch").await).await;
    }

    #[tokio::test]
    async fn a_draft18_fetched_object_with_malformed_immutable_properties_is_a_malformed_track() {
        assert_fetch_object_is_malformed(
            published_track_over("immutable-fetch-d18", Version::Draft18).await,
        )
        .await;
    }

    async fn assert_well_formed_object_is_delivered(
        track: PublishedTrack,
        contents: &'static [u8],
    ) {
        // Arrange
        let object = Object {
            extension_headers: with_immutable(contents),
            ..object_with_extensions()
        };
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        within(send_subgroup_object(
            &track.client,
            &track.published,
            &object,
        ))
        .await;
        let mut stream = within(next_subgroup(&mut receiver)).await;
        within(stream.receive()).await.unwrap();

        // Act
        let received = within(stream.receive()).await;

        // Assert
        assert!(
            matches!(&received, Ok(Some(Subgroup::Object(delivered)))
                if delivered.extension_headers == object.extension_headers),
            "{received:?}"
        );
    }

    #[tokio::test]
    async fn an_object_with_well_formed_immutable_properties_is_delivered() {
        let track = published_track("immutable-well-formed").await;
        assert_well_formed_object_is_delivered(track, WELL_FORMED_DRAFT14).await;
    }

    #[tokio::test]
    async fn a_draft18_object_with_well_formed_immutable_properties_is_delivered() {
        let track = published_track_over("immutable-well-formed-d18", Version::Draft18).await;
        assert_well_formed_object_is_delivered(track, WELL_FORMED_DRAFT18).await;
    }
}
