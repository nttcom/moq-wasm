use std::mem;

use anyhow::{Context, bail, ensure};
use bytes::Bytes;

use crate::{
    object::Location,
    object::{
        DataStreamResetCode, ExtensionHeaders, ForwardingPreference, Object, ObjectStatus,
        StreamDataSenderFactory, StreamPriority, SubgroupObjectSender, SubgroupStart,
    },
};

/// The default Subscriber and Publisher Priority: the middle of their range
/// (draft-14 §7.1).
pub(crate) const DEFAULT_PRIORITY: u8 = 128;
const NO_GROUP_AFTER_MAX: &str = "no group follows group u64::MAX";

pub struct TrackWriter {
    factory: StreamDataSenderFactory,
    numbering: GroupNumbering,
    slot: GroupSlot,
    opened_streams: u64,
    subscriber_priority: u8,
    publisher_priority: u8,
}

impl TrackWriter {
    pub fn new(factory: StreamDataSenderFactory, first_group_id: u64) -> Self {
        Self {
            factory,
            numbering: GroupNumbering::new(first_group_id),
            slot: GroupSlot::NoGroup,
            opened_streams: 0,
            subscriber_priority: DEFAULT_PRIORITY,
            publisher_priority: DEFAULT_PRIORITY,
        }
    }

    /// Each stream it opens carries `publisher_priority` in its header and is
    /// scheduled on the transport by both priorities (draft-14 §7.2).
    pub fn with_priorities(mut self, subscriber_priority: u8, publisher_priority: u8) -> Self {
        self.subscriber_priority = subscriber_priority;
        self.publisher_priority = publisher_priority;
        self
    }

    /// Leaves the open group because one of its objects is not sent, as in
    /// Forward State 0 (draft-18 §5.1). Its stream is reset once the next
    /// stream opens or the writer ends: a FIN would tell the receiver it has
    /// the whole subgroup (draft-14 §10.4.3).
    pub(crate) fn abandon_group(&mut self) {
        self.slot.abandon();
    }

    pub(crate) fn abandoned_group_id(&self) -> Option<u64> {
        self.slot.abandoned_id()
    }

    /// Takes the group left by `abandon_group` up again on a new stream
    /// whose first object id is `object_id`, which draft-18 §11.4.3 allows
    /// once the Forward State is 1 again. Does nothing without such a group.
    pub(crate) async fn rejoin_group_at(&mut self, object_id: u64) -> anyhow::Result<()> {
        let Some(group_id) = self.abandoned_group_id() else {
            return Ok(());
        };
        self.reset_abandoned_group().await;
        self.numbering.rewind_to(group_id);
        let mut group = self.open_group().await?;
        group.next_object_id = object_id;
        self.slot = GroupSlot::Open(group);
        Ok(())
    }

    pub async fn start_group(&mut self) -> anyhow::Result<()> {
        self.finish_current_group().await;
        self.slot = GroupSlot::Open(self.open_group().await?);
        Ok(())
    }

    /// Opens a group with a chosen id so tracks of a switching set can start
    /// their groups at the same ids. Ids skipped over are announced on the new
    /// group's first object with the Prior Group ID Gap extension header of
    /// draft-ietf-moq-transport-14; a writer that has not opened a group yet
    /// may start anywhere.
    pub async fn start_group_at(&mut self, group_id: u64) -> anyhow::Result<()> {
        self.numbering.skip_to(group_id)?;
        self.start_group().await
    }

    /// Opens `group_id` on a new stream whose first object id is `object_id`, so
    /// a subscription can start in the middle of a group. Call it only while no
    /// group is open.
    pub(crate) async fn join_group_at(
        &mut self,
        group_id: u64,
        object_id: u64,
    ) -> anyhow::Result<()> {
        self.numbering.skip_to(group_id)?;
        let mut group = self.open_group().await?;
        group.next_object_id = object_id;
        self.slot = GroupSlot::Open(group);
        Ok(())
    }

    pub(crate) async fn write_group_at(
        &mut self,
        group_id: u64,
        payload: Bytes,
    ) -> anyhow::Result<()> {
        self.numbering.skip_to(group_id)?;
        self.write_group(payload).await
    }

    /// Each of `immutable_extensions` is the Key-Value-Pairs of one
    /// Immutable Extensions header; a receiver treats a track whose header
    /// holds anything else as malformed (draft-14 §11.2, draft-18 §12.7).
    pub async fn write(
        &mut self,
        payload: Bytes,
        immutable_extensions: Vec<Bytes>,
    ) -> anyhow::Result<()> {
        self.write_with_extension_headers(
            payload,
            ExtensionHeaders::from_immutable_extensions(immutable_extensions),
        )
        .await
    }

    pub async fn write_with_extension_headers(
        &mut self,
        payload: Bytes,
        extension_headers: ExtensionHeaders,
    ) -> anyhow::Result<()> {
        let extension_headers = self.with_pending_group_gap(extension_headers);
        let GroupSlot::Open(group) = &mut self.slot else {
            bail!("write before start_group");
        };
        let result = group.write_object(payload, extension_headers).await;
        if result.is_err() {
            self.slot = GroupSlot::NoGroup;
        }
        result
    }

    pub async fn write_group(&mut self, payload: Bytes) -> anyhow::Result<()> {
        self.finish_current_group().await;
        let mut group = self.open_group().await?;
        let extension_headers = self.with_pending_group_gap(ExtensionHeaders::default());
        group.write_object(payload, extension_headers).await?;
        group.finish().await
    }

    /// Resets the open group's stream with CANCELLED instead of finishing it:
    /// draft-ietf-moq-transport-14 §10.4.3 forbids a FIN when the subscription
    /// ends before the group, because a FIN tells the receiver it has the whole
    /// subgroup.
    pub(crate) async fn cancel(mut self) {
        self.abandon_group();
        self.reset_abandoned_group().await;
    }

    async fn reset_abandoned_group(&mut self) {
        if let Some(mut group) = self.slot.take_abandoned()
            && let Err(error) = group.sender.reset(DataStreamResetCode::Cancelled).await
        {
            tracing::debug!(?error, "open group could not be reset");
        }
    }

    pub async fn finish(mut self) -> anyhow::Result<()> {
        self.reset_abandoned_group().await;
        match self.slot.take_open() {
            Some(group) => group.finish().await,
            None => Ok(()),
        }
    }

    /// The Stream Count of draft-ietf-moq-transport-14 §9.12.
    pub(crate) fn opened_streams(&self) -> u64 {
        self.opened_streams
    }

    /// The number of Group IDs from the first group to the last one opened,
    /// saturating at u64::MAX when that span is all 2^64 of them.
    pub fn groups(&self) -> u64 {
        self.numbering.groups()
    }

    pub fn current_group_id(&self) -> Option<u64> {
        self.slot.open_id()
    }

    fn with_pending_group_gap(
        &mut self,
        mut extension_headers: ExtensionHeaders,
    ) -> ExtensionHeaders {
        if let Some(gap) = self.numbering.take_gap() {
            extension_headers.push_prior_group_id_gap(gap);
        }
        extension_headers
    }

    /// A group the subscriber stopped or the transport reset cannot be
    /// finished, and its stream is gone either way, so the next group opens
    /// on a new stream regardless.
    async fn finish_current_group(&mut self) {
        self.reset_abandoned_group().await;
        if let Some(group) = self.slot.take_open()
            && let Err(error) = group.finish().await
        {
            tracing::debug!(?error, "previous group could not be finished");
        }
    }

    async fn open_group(&mut self) -> anyhow::Result<GroupSender> {
        let group_id = self.numbering.next()?;
        let stream = self.factory.next().await?;
        self.opened_streams += 1;
        stream
            .set_priority(StreamPriority {
                subscriber_priority: self.subscriber_priority,
                publisher_priority: self.publisher_priority,
                group_position: 0,
                subgroup_id: 0,
            })
            .await?;
        let sender = stream
            .start(SubgroupStart {
                group_id,
                subgroup_id: 0,
                publisher_priority: self.publisher_priority,
                ends_group: false,
            })
            .await?;
        self.numbering.opened(group_id);
        Ok(GroupSender {
            sender,
            group_id,
            next_object_id: 0,
            publisher_priority: self.publisher_priority,
        })
    }
}

/// The group a writer holds (track_writer.qnt `Slot`): at most one, open or
/// abandoned.
enum GroupSlot {
    NoGroup,
    Open(GroupSender),
    /// Left by `abandon_group`; its stream is reset, never finished.
    Abandoned(GroupSender),
}

impl GroupSlot {
    fn open_id(&self) -> Option<u64> {
        match self {
            Self::Open(group) => Some(group.group_id),
            Self::NoGroup | Self::Abandoned(_) => None,
        }
    }

    fn abandoned_id(&self) -> Option<u64> {
        match self {
            Self::Abandoned(group) => Some(group.group_id),
            Self::NoGroup | Self::Open(_) => None,
        }
    }

    fn abandon(&mut self) {
        *self = match mem::replace(self, Self::NoGroup) {
            Self::Open(group) => Self::Abandoned(group),
            other => other,
        };
    }

    fn take_open(&mut self) -> Option<GroupSender> {
        match mem::replace(self, Self::NoGroup) {
            Self::Open(group) => Some(group),
            other => {
                *self = other;
                None
            }
        }
    }

    fn take_abandoned(&mut self) -> Option<GroupSender> {
        match mem::replace(self, Self::NoGroup) {
            Self::Abandoned(group) => Some(group),
            other => {
                *self = other;
                None
            }
        }
    }
}

/// The Group IDs a writer opened and the Prior Group ID Gap its next object
/// carries (track_writer.qnt `Writer` `first`, `next`, `pendingGap`).
struct GroupNumbering {
    first: u64,
    /// `None` once group u64::MAX opened: no group follows it.
    next: Option<u64>,
    /// Never `Some(0)`.
    pending_gap: Option<u64>,
}

impl GroupNumbering {
    fn new(first: u64) -> Self {
        Self {
            first,
            next: Some(first),
            pending_gap: None,
        }
    }

    /// Moves the next group to `group_id`, keeping a gap past the next id
    /// for the next object; before any group opened it moves the first one
    /// too. Fails, changing nothing, when `group_id` precedes the next group.
    fn skip_to(&mut self, group_id: u64) -> anyhow::Result<()> {
        if self.groups() == 0 {
            self.first = group_id;
        } else {
            let next = self.next()?;
            ensure!(
                group_id >= next,
                "group {group_id} precedes the next group {next}"
            );
            if group_id > next {
                self.pending_gap = Some(group_id - next);
            }
        }
        self.next = Some(group_id);
        Ok(())
    }

    fn next(&self) -> anyhow::Result<u64> {
        self.next.context(NO_GROUP_AFTER_MAX)
    }

    fn opened(&mut self, group_id: u64) {
        self.next = group_id.checked_add(1);
    }

    /// Makes an abandoned group the next one again to reopen it.
    fn rewind_to(&mut self, group_id: u64) {
        self.next = Some(group_id);
    }

    fn take_gap(&mut self) -> Option<u64> {
        self.pending_gap.take()
    }

    fn groups(&self) -> u64 {
        match self.next {
            Some(next) => next - self.first,
            None => (u64::MAX - self.first).saturating_add(1),
        }
    }
}

struct GroupSender {
    sender: SubgroupObjectSender,
    group_id: u64,
    next_object_id: u64,
    publisher_priority: u8,
}

impl GroupSender {
    async fn write_object(
        &mut self,
        payload: Bytes,
        extension_headers: ExtensionHeaders,
    ) -> anyhow::Result<()> {
        self.send(ObjectStatus::Normal, extension_headers, payload)
            .await
    }

    async fn finish(mut self) -> anyhow::Result<()> {
        self.send(
            ObjectStatus::EndOfGroup,
            ExtensionHeaders::default(),
            Bytes::new(),
        )
        .await?;
        self.sender.close().await
    }

    async fn send(
        &mut self,
        status: ObjectStatus,
        extension_headers: ExtensionHeaders,
        payload: Bytes,
    ) -> anyhow::Result<()> {
        let object = Object {
            location: Location {
                group_id: self.group_id,
                object_id: self.next_object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: self.publisher_priority,
            status,
            extension_headers,
            payload,
            ends_group: false,
        };
        self.sender.send_object(&object).await?;
        self.next_object_id += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{
        Session, Subscription, TrackWriter, Version,
        modules::test_support::{
            DRAFTS, accept_publish, connected_sessions, data_receiver, next_subgroup,
            subscribed_track_reader, within,
        },
        object::{ExtensionHeaders, KeyValuePair, Subgroup, VariantType},
        publish::PublishOption,
    };

    /// A draft-18 writer of a track the client published and the server
    /// accepted: the client, the server, the server's subscription and the
    /// writer.
    async fn published_writer(
        name: &str,
        first_group_id: u64,
    ) -> (Session, Session, Subscription, TrackWriter) {
        let (client, server) = connected_sessions(name, Version::Draft18).await;
        let publisher = client.publisher();
        let (published, accepted) = within(async {
            tokio::join!(
                publisher.publish("ns".into(), "track".into(), PublishOption::default()),
                accept_publish(&server)
            )
        })
        .await;
        let writer = TrackWriter::new(publisher.create_stream(&published.unwrap()), first_group_id);
        (client, server, accepted, writer)
    }

    #[tokio::test]
    async fn objects_arrive_with_their_immutable_extensions_and_ids() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("track-writer", version).await;
            let publisher = client.publisher();
            let (published, accepted) = within(async {
                tokio::join!(
                    publisher.publish("ns".into(), "track".into(), PublishOption::default()),
                    accept_publish(&server)
                )
            })
            .await;
            let mut writer = TrackWriter::new(publisher.create_stream(&published.unwrap()), 7);

            // Act
            writer.start_group().await.unwrap();
            writer
                .write(
                    Bytes::from_static(b"with"),
                    vec![Bytes::from_static(b"\x02\x05")],
                )
                .await
                .unwrap();
            writer
                .write(Bytes::from_static(b"without"), vec![])
                .await
                .unwrap();
            writer
                .write_with_extension_headers(
                    Bytes::from_static(b"keyed"),
                    ExtensionHeaders::new(vec![KeyValuePair {
                        key: 2,
                        value: VariantType::Even(42),
                    }]),
                )
                .await
                .unwrap();
            let current_group_id = writer.current_group_id();
            writer.finish().await.unwrap();
            let mut reader = subscribed_track_reader(&server, &accepted).await;
            let first = reader.next_object().await.unwrap().unwrap();
            let second = reader.next_object().await.unwrap().unwrap();
            let third = reader.next_object().await.unwrap().unwrap();

            // Assert
            assert_eq!(current_group_id, Some(7));
            assert_eq!((first.group_id, first.object_id), (7, 0));
            assert_eq!(first.payload, Bytes::from_static(b"with"));
            assert_eq!(
                first.extension_headers.immutable_extensions(),
                vec![Bytes::from_static(b"\x02\x05")]
            );
            assert_eq!((second.group_id, second.object_id), (7, 1));
            assert_eq!(second.payload, Bytes::from_static(b"without"));
            assert!(second.extension_headers.immutable_extensions().is_empty());
            assert_eq!((third.group_id, third.object_id), (7, 2));
            assert_eq!(
                third.extension_headers.key_value_pairs,
                vec![KeyValuePair {
                    key: 2,
                    value: VariantType::Even(42),
                }]
            );
        }
    }

    #[tokio::test]
    async fn chosen_group_ids_announce_the_ids_they_skip() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("track-writer-groups", version).await;
            let publisher = client.publisher();
            let (published, accepted) = within(async {
                tokio::join!(
                    publisher.publish("ns".into(), "track".into(), PublishOption::default()),
                    accept_publish(&server)
                )
            })
            .await;
            let mut writer = TrackWriter::new(publisher.create_stream(&published.unwrap()), 7);

            // Act: the first group may start anywhere, the next one skips 4 and 5
            writer.start_group_at(3).await.unwrap();
            writer
                .write(Bytes::from_static(b"a"), vec![])
                .await
                .unwrap();
            writer.start_group_at(6).await.unwrap();
            writer
                .write(Bytes::from_static(b"b"), vec![])
                .await
                .unwrap();
            writer
                .write(Bytes::from_static(b"c"), vec![])
                .await
                .unwrap();
            let current_group_id = writer.current_group_id();
            writer.finish().await.unwrap();
            let mut reader = subscribed_track_reader(&server, &accepted).await;
            let first = reader.next_object().await.unwrap().unwrap();
            let second = reader.next_object().await.unwrap().unwrap();
            let third = reader.next_object().await.unwrap().unwrap();

            // Assert
            assert_eq!(current_group_id, Some(6));
            assert_eq!((first.group_id, first.object_id), (3, 0));
            assert!(first.extension_headers.prior_group_id_gap().is_empty());
            assert_eq!((second.group_id, second.object_id), (6, 0));
            assert_eq!(second.extension_headers.prior_group_id_gap(), vec![2]);
            assert_eq!((third.group_id, third.object_id), (6, 1));
            assert!(third.extension_headers.prior_group_id_gap().is_empty());
        }
    }

    #[tokio::test]
    async fn the_largest_group_id_opens_and_no_group_follows_it() {
        // Arrange
        let (_client, server, accepted, mut writer) =
            published_writer("track-writer-largest-group-id", 0).await;

        // Act
        writer.start_group_at(0).await.unwrap();
        writer
            .write(Bytes::from_static(b"a"), vec![])
            .await
            .unwrap();
        writer.start_group_at(u64::MAX).await.unwrap();
        writer
            .write(Bytes::from_static(b"b"), vec![])
            .await
            .unwrap();
        let after = writer.start_group().await;
        let chosen_after = writer.start_group_at(u64::MAX).await;
        let groups = writer.groups();
        writer.finish().await.unwrap();
        let mut reader = subscribed_track_reader(&server, &accepted).await;
        let first = reader.next_object().await.unwrap().unwrap();
        let last = reader.next_object().await.unwrap().unwrap();

        // Assert: `groups` saturates at the span of all 2^64 Group IDs
        assert!(after.is_err());
        assert!(chosen_after.is_err());
        assert_eq!(groups, u64::MAX);
        assert_eq!((first.group_id, first.object_id), (0, 0));
        assert_eq!((last.group_id, last.object_id), (u64::MAX, 0));
        assert_eq!(
            last.extension_headers.prior_group_id_gap(),
            vec![u64::MAX - 1]
        );
    }

    #[tokio::test]
    async fn next_group_reaches_the_largest_group_id() {
        // Arrange
        let (_client, server, accepted, mut writer) =
            published_writer("track-writer-next-largest-group-id", u64::MAX - 1).await;

        // Act
        writer.write_group(Bytes::from_static(b"a")).await.unwrap();
        writer.start_group_at(u64::MAX).await.unwrap();
        writer
            .write(Bytes::from_static(b"b"), vec![])
            .await
            .unwrap();
        let after = writer.write_group(Bytes::from_static(b"c")).await;
        let groups = writer.groups();
        writer.finish().await.unwrap();
        let mut reader = subscribed_track_reader(&server, &accepted).await;
        let first = reader.next_object().await.unwrap().unwrap();
        let last = reader.next_object().await.unwrap().unwrap();

        // Assert: choosing the next Group ID skips none, so no gap is announced
        assert!(after.is_err());
        assert_eq!(groups, 2);
        assert_eq!(first.group_id, u64::MAX - 1);
        assert_eq!(last.group_id, u64::MAX);
        assert!(last.extension_headers.prior_group_id_gap().is_empty());
    }

    #[tokio::test]
    async fn a_group_the_subscriber_stopped_does_not_keep_the_next_group_from_opening() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("track-writer-stopped-group", version).await;
            let publisher = client.publisher();
            let (published, accepted) = within(async {
                tokio::join!(
                    publisher.publish("ns".into(), "track".into(), PublishOption::default()),
                    accept_publish(&server)
                )
            })
            .await;
            let mut writer = TrackWriter::new(publisher.create_stream(&published.unwrap()), 7);
            writer.start_group().await.unwrap();
            writer
                .write(Bytes::from_static(b"a"), vec![])
                .await
                .unwrap();
            let mut streams = data_receiver(&server, &accepted).await;
            drop(next_subgroup(&mut streams).await);
            within(async {
                while writer
                    .write(Bytes::from_static(b"after stop"), vec![])
                    .await
                    .is_ok()
                {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;

            // Act
            writer.start_group().await.unwrap();
            writer
                .write(Bytes::from_static(b"b"), vec![])
                .await
                .unwrap();
            let mut next_stream = next_subgroup(&mut streams).await;
            let header = next_stream.receive().await.unwrap();
            let object = next_stream.receive().await.unwrap();

            // Assert
            let Some(Subgroup::Header(header)) = header else {
                panic!("expected the next group's subgroup header");
            };
            assert_eq!(header.group_id, 8);
            let Some(Subgroup::Object(object)) = object else {
                panic!("expected the next group's first object");
            };
            assert_eq!(object.payload, Bytes::from_static(b"b"));
        }
    }

    mod properties {
        use proptest::{collection::vec, prelude::*};

        use super::super::*;

        /// Group IDs clustered at 0 and at u64::MAX, so a writer skips
        /// forward, steps back and runs out of IDs.
        fn group_id() -> impl Strategy<Value = u64> {
            prop_oneof![0..4u64, u64::MAX - 3..=u64::MAX]
        }

        #[derive(Clone, Debug)]
        enum Step {
            SkipTo(u64),
            Open,
            /// `rejoin_group_at`: back to the group opened last, which only
            /// an abandoned group still open since the last skip can be.
            Rewind,
            TakeGap,
        }

        fn step() -> impl Strategy<Value = Step> {
            prop_oneof![
                group_id().prop_map(Step::SkipTo),
                Just(Step::Open),
                Just(Step::Rewind),
                Just(Step::TakeGap),
            ]
        }

        /// track_writer.qnt `skipTo` / `opened` / `written` under
        /// `track_writer_current`, in u128 so the id after u64::MAX exists.
        struct Model {
            first: u128,
            next: u128,
            pending_gap: u128,
        }

        const EXHAUSTED: u128 = 1 << 64;

        impl Model {
            fn skip_to(&mut self, group_id: u128) -> bool {
                if self.next == self.first {
                    self.first = group_id;
                } else if self.next == EXHAUSTED || group_id < self.next {
                    return false;
                } else if group_id > self.next {
                    self.pending_gap = group_id - self.next;
                }
                self.next = group_id;
                true
            }

            fn groups(&self) -> u64 {
                u64::try_from(self.next - self.first).unwrap_or(u64::MAX)
            }
        }

        proptest! {
            #[test]
            fn group_numbering_follows_the_model(
                first_group_id in group_id(),
                steps in vec(step(), 1..32),
            ) {
                // Arrange
                let mut numbering = GroupNumbering::new(first_group_id);
                let mut model = Model { first: first_group_id.into(), next: first_group_id.into(), pending_gap: 0 };
                let mut rejoinable = None;

                for step in steps {
                    // Act
                    let (succeeded, expected) = match step {
                        Step::SkipTo(group_id) => {
                            rejoinable = None;
                            (numbering.skip_to(group_id).is_ok(), model.skip_to(group_id.into()))
                        }
                        Step::Open => {
                            let opened = numbering.next().map(|group_id| {
                                numbering.opened(group_id);
                                rejoinable = Some(group_id);
                            });
                            let expected = model.next < EXHAUSTED;
                            if expected {
                                model.next += 1;
                            }
                            (opened.is_ok(), expected)
                        }
                        Step::Rewind => {
                            if let Some(group_id) = rejoinable.take() {
                                numbering.rewind_to(group_id);
                                model.next = group_id.into();
                            }
                            (true, true)
                        }
                        Step::TakeGap => {
                            let gap = numbering.take_gap();
                            let expected = (model.pending_gap > 0).then(|| u64::try_from(model.pending_gap).unwrap());
                            model.pending_gap = 0;
                            (gap == expected, true)
                        }
                    };

                    // Assert: same outcome, Group IDs and gap as the model; a gap is never `Some(0)`
                    prop_assert_eq!(succeeded, expected, "{:?}", step);
                    prop_assert_eq!(numbering.groups(), model.groups());
                    prop_assert_eq!(numbering.next, u64::try_from(model.next).ok());
                    prop_assert_eq!(numbering.pending_gap, (model.pending_gap > 0).then(|| u64::try_from(model.pending_gap).unwrap()));
                }
            }
        }
    }
}
