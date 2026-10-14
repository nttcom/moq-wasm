use std::sync::{Arc, Weak};

use anyhow::Result;
use tokio::sync::Mutex;

use super::{FETCH_PUBLISHER_PRIORITY, FETCH_SUBGROUP_ID, OriginState, object_cache::CachedObject};
use crate::{
    Session,
    fetch::FetchHandler,
    modules::executor::{self, JoinHandle},
    object::{ForwardingPreference, GroupOrder, Location, Object, ObjectStatus},
};

pub(super) struct FetchServeTask {
    handle: JoinHandle,
}

impl FetchServeTask {
    pub(super) fn run(
        session: Arc<Session>,
        state: Weak<Mutex<OriginState>>,
        handler: FetchHandler,
        objects: Vec<CachedObject>,
        end_location: Location,
    ) -> Self {
        let handle = executor::spawn("Origin FETCH Serve", async move {
            let request_id = handler.request_id;
            if let Err(error) = respond(&session, &handler, objects, end_location).await {
                tracing::warn!(request_id, ?error, "failed to serve FETCH");
            }
            if let Some(state) = state.upgrade() {
                // Removing the entry aborts this very task, harmlessly: nothing is awaited after it.
                state.lock().await.fetches.remove(&request_id);
            }
        });
        Self { handle }
    }
}

impl Drop for FetchServeTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn respond(
    session: &Session,
    handler: &FetchHandler,
    objects: Vec<CachedObject>,
    end_location: Location,
) -> Result<()> {
    handler.ok(false, end_location).await?;
    let sender = session
        .publisher()
        .create_fetch_stream(handler.request_id)
        .await?;
    for object in in_group_order(objects, handler.group_order) {
        sender
            .send_object(&Object {
                location: object.location,
                forwarding: ForwardingPreference::Subgroup {
                    subgroup_id: FETCH_SUBGROUP_ID,
                },
                publisher_priority: FETCH_PUBLISHER_PRIORITY,
                status: ObjectStatus::Normal,
                extension_headers: object.extension_headers,
                payload: object.payload,
                ends_group: false,
            })
            .await?;
    }
    sender.close().await
}

fn in_group_order(mut objects: Vec<CachedObject>, order: GroupOrder) -> Vec<CachedObject> {
    if order == GroupOrder::Descending {
        objects.sort_by(|a, b| {
            b.location
                .group_id
                .cmp(&a.location.group_id)
                .then(a.location.object_id.cmp(&b.location.object_id))
        });
    }
    objects
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::object::ExtensionHeaders;

    fn object(group_id: u64, object_id: u64) -> CachedObject {
        CachedObject {
            location: Location {
                group_id,
                object_id,
            },
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::new(),
        }
    }

    fn ids(objects: Vec<CachedObject>) -> Vec<(u64, u64)> {
        objects
            .iter()
            .map(|object| (object.location.group_id, object.location.object_id))
            .collect()
    }

    #[test]
    fn descending_order_reverses_the_groups_but_keeps_the_objects_ascending() {
        // Arrange
        let objects = vec![object(7, 0), object(7, 1), object(8, 0), object(8, 1)];

        // Act
        let ascending = in_group_order(objects.clone(), GroupOrder::Ascending);
        let descending = in_group_order(objects, GroupOrder::Descending);

        // Assert
        assert_eq!(ids(ascending), [(7, 0), (7, 1), (8, 0), (8, 1)]);
        assert_eq!(ids(descending), [(8, 0), (8, 1), (7, 0), (7, 1)]);
    }
}
