use super::{
    cache::{CacheId, CacheKey},
    commands::ObjectCacheStorageCommand,
};
use anyhow::{Result, bail};
use moqt_core::messages::data_streams::{DatagramObject, subgroup_stream};
use tokio::sync::{mpsc, oneshot};

pub(crate) struct ObjectCacheStorageWrapper {
    tx: mpsc::Sender<ObjectCacheStorageCommand>,
}

impl ObjectCacheStorageWrapper {
    pub fn new(tx: mpsc::Sender<ObjectCacheStorageCommand>) -> Self {
        Self { tx }
    }

    pub(crate) async fn create_datagram_cache(&mut self, cache_key: &CacheKey) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::CreateDatagramCache {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn create_subgroup_stream_cache(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        header: subgroup_stream::Header,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::CreateSubgroupStreamCache {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            header,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn exist_datagram_cache(&mut self, cache_key: &CacheKey) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();

        let cmd = ObjectCacheStorageCommand::HasDatagramCache {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(exist) => Ok(exist),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_subgroup_stream_header(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<subgroup_stream::Header> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<subgroup_stream::Header>>();

        let cmd = ObjectCacheStorageCommand::GetSubgroupStreamHeader {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(header_cache) => Ok(header_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn set_datagram_object(
        &mut self,
        cache_key: &CacheKey,
        datagram_object: DatagramObject,
        duration: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::SetDatagramObject {
            cache_key: cache_key.clone(),
            datagram_object,
            duration,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn set_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        subgroup_stream_object: subgroup_stream::Object,
        duration: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::SetSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            subgroup_stream_object,
            duration,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_absolute_datagram_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        object_id: u64,
    ) -> Result<Option<(CacheId, DatagramObject)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, DatagramObject)>>>();

        let cmd = ObjectCacheStorageCommand::GetDatagramObject {
            cache_key: cache_key.clone(),
            group_id,
            object_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_absolute_or_next_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
    ) -> Result<Option<(CacheId, subgroup_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, subgroup_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetAbsoluteOrNextSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            object_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_next_datagram_object(
        &mut self,
        cache_key: &CacheKey,
        cache_id: usize,
    ) -> Result<Option<(CacheId, DatagramObject)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, DatagramObject)>>>();

        let cmd = ObjectCacheStorageCommand::GetNextDatagramObject {
            cache_key: cache_key.clone(),
            cache_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_next_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        cache_id: usize,
    ) -> Result<Option<(CacheId, subgroup_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, subgroup_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetNextSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            cache_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_latest_datagram_object(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<(CacheId, DatagramObject)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, DatagramObject)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestDatagramObject {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_latest_datagram_group(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<(CacheId, DatagramObject)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, DatagramObject)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestDatagramGroup {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_first_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<Option<(CacheId, subgroup_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, subgroup_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetFirstSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn get_latest_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<Option<(CacheId, subgroup_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, subgroup_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_all_subgroup_ids(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
    ) -> Result<Vec<u64>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Vec<u64>>>();

        let cmd = ObjectCacheStorageCommand::GetAllSubgroupIds {
            cache_key: cache_key.clone(),
            group_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(subgroup_ids) => Ok(subgroup_ids),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_largest_group_id(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<u64>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<u64>>>();

        let cmd = ObjectCacheStorageCommand::GetLargestGroupId {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(group_id) => Ok(group_id),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_largest_object_id(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<u64>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<u64>>>();

        let cmd = ObjectCacheStorageCommand::GetLargestObjectId {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_id) => Ok(object_id),
            Err(err) => bail!(err),
        }
    }

    pub async fn delete_client(&mut self, session_id: usize) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::DeleteClient {
            session_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
}

#[cfg(test)]
mod success {
    use crate::modules::object_cache_storage::{
        cache::CacheKey, commands::ObjectCacheStorageCommand, storage::object_cache_storage,
        wrapper::ObjectCacheStorageWrapper,
    };
    use moqt_core::messages::data_streams::{
        DatagramObject, datagram, datagram_status, object_status::ObjectStatus, subgroup_stream,
    };
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn create_datagram_cache() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage.create_datagram_cache(&cache_key).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_subgroup_stream_cache() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 2;
        let group_id = 3;
        let subgroup_id = 4;
        let publisher_priority = 5;

        let subgroup_stream_header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, subgroup_stream_header)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn exist_datagram_cache() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage.exist_datagram_cache(&cache_key).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        let result = object_cache_storage.exist_datagram_cache(&cache_key).await;

        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn get_subgroup_stream_header() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let group_id = 3;
        let subgroup_id = 4;
        let publisher_priority = 5;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_subgroup_stream_header(&cache_key, group_id, subgroup_id)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), header);
    }

    #[tokio::test]
    async fn set_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let object_id = 2;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let object_payload = vec![1, 2, 3, 4];
        let duration = 1000;
        let datagram_object = datagram::Object::new(
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers,
            object_payload,
        )
        .unwrap();
        let datagram_object = DatagramObject::ObjectDatagram(datagram_object);

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;
        let result = object_cache_storage
            .set_datagram_object(&cache_key, datagram_object, duration)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_datagram_object_status() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let object_id = 2;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let object_status = ObjectStatus::EndOfGroup;
        let duration = 1000;
        let datagram_object = datagram_status::Object::new(
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers,
            object_status,
        )
        .unwrap();
        let datagram_object = DatagramObject::ObjectDatagramStatus(datagram_object);

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;
        let result = object_cache_storage
            .set_datagram_object(&cache_key, datagram_object, duration)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let object_id = 2;
        let group_id = 3;
        let subgroup_id = 4;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let object_status = None;
        let object_payload = vec![1, 2, 3, 4];
        let subgroup_stream_object = subgroup_stream::Object::new(
            object_id,
            extension_headers,
            object_status,
            object_payload,
        )
        .unwrap();
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;
        let result = object_cache_storage
            .set_subgroup_stream_object(
                &cache_key,
                group_id,
                subgroup_id,
                subgroup_stream_object,
                duration,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_absolute_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers.clone(),
                object_payload,
            )
            .unwrap();
            let datagram_object = DatagramObject::ObjectDatagram(datagram_object);

            let _ = object_cache_storage
                .set_datagram_object(&cache_key, datagram_object, duration)
                .await;
        }

        let object_id = 5;
        let expected_cache_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_object = datagram::Object::new(
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            extension_headers,
            expected_object_payload,
        )
        .unwrap();
        let expected_object = DatagramObject::ObjectDatagram(expected_object);

        let result = object_cache_storage
            .get_absolute_datagram_object(&cache_key, group_id, object_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_absolute_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let extension_headers = vec![];
        let object_status = None;
        let duration = 1000;
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers.clone(),
                object_status,
                object_payload,
            )
            .unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let object_id = 9;
        let expected_cache_id = 9;
        let expected_object_payload = vec![9, 10, 11, 12];
        let expected_object = subgroup_stream::Object::new(
            object_id,
            extension_headers,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_absolute_or_next_subgroup_stream_object(
                &cache_key,
                group_id,
                subgroup_id,
                object_id,
            )
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_next_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers.clone(),
                object_payload,
            )
            .unwrap();
            let datagram_object = DatagramObject::ObjectDatagram(datagram_object);

            let _ = object_cache_storage
                .set_datagram_object(&cache_key, datagram_object, duration)
                .await;
        }

        let cache_id = 2;
        let expected_object_id = 3;
        let expected_cache_id = 3;
        let expected_object_payload = vec![3, 4, 5, 6];
        let expected_object = datagram::Object::new(
            track_alias,
            group_id,
            expected_object_id,
            publisher_priority,
            extension_headers,
            expected_object_payload,
        )
        .unwrap();
        let expected_object = DatagramObject::ObjectDatagram(expected_object);

        let result = object_cache_storage
            .get_next_datagram_object(&cache_key, cache_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_next_datagram_object_status() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for i in 0..10 {
            let object_status = ObjectStatus::DoesNotExist;
            let object_id = i as u64;

            let datagram_object = datagram_status::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers.clone(),
                object_status,
            )
            .unwrap();
            let datagram_object = DatagramObject::ObjectDatagramStatus(datagram_object);

            let _ = object_cache_storage
                .set_datagram_object(&cache_key, datagram_object, duration)
                .await;
        }

        let cache_id = 2;
        let expected_object_id = 3;
        let expected_cache_id = 3;
        let expected_object_status = ObjectStatus::DoesNotExist;
        let expected_object = datagram_status::Object::new(
            track_alias,
            group_id,
            expected_object_id,
            publisher_priority,
            extension_headers,
            expected_object_status,
        )
        .unwrap();
        let expected_object = DatagramObject::ObjectDatagramStatus(expected_object);

        let result = object_cache_storage
            .get_next_datagram_object(&cache_key, cache_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_next_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let extension_headers = vec![];
        let object_status = None;
        let duration = 1000;
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers.clone(),
                object_status,
                object_payload,
            )
            .unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let cache_id = 0;
        let expected_object_id = 1;
        let expected_cache_id = 1;
        let expected_object_payload = vec![1, 2, 3, 4];
        let expected_object = subgroup_stream::Object::new(
            expected_object_id,
            extension_headers,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_next_subgroup_stream_object(&cache_key, group_id, subgroup_id, cache_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for i in 0..6 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers.clone(),
                object_payload,
            )
            .unwrap();
            let datagram_object = DatagramObject::ObjectDatagram(datagram_object);

            let _ = object_cache_storage
                .set_datagram_object(&cache_key, datagram_object, duration)
                .await;
        }

        let expected_object_id = 5;
        let expected_cache_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_object = datagram::Object::new(
            track_alias,
            group_id,
            expected_object_id,
            publisher_priority,
            extension_headers,
            expected_object_payload,
        )
        .unwrap();
        let expected_object = DatagramObject::ObjectDatagram(expected_object);

        let result = object_cache_storage
            .get_latest_datagram_object(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_group_ascending_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        let group_size = 7;
        for j in 0..4 {
            let group_id = j as u64;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram_object = datagram::Object::new(
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    extension_headers.clone(),
                    object_payload,
                )
                .unwrap();
                let datagram_object = DatagramObject::ObjectDatagram(datagram_object);

                let _ = object_cache_storage
                    .set_datagram_object(&cache_key, datagram_object, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 3;
        let expected_object_payload = vec![21, 22, 23, 24];
        let expected_object = datagram::Object::new(
            track_alias,
            expected_group_id,
            expected_object_id,
            publisher_priority,
            extension_headers,
            expected_object_payload,
        )
        .unwrap();
        let expected_object = DatagramObject::ObjectDatagram(expected_object);
        let expected_cache_id = group_size * expected_group_id as u8 + expected_object_id as u8;

        let result = object_cache_storage
            .get_latest_datagram_group(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id as usize);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_group_descending_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        let group_size = 7;
        for j in (2..10).rev() {
            let group_id = j as u64;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram_object = datagram::Object::new(
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    extension_headers.clone(),
                    object_payload,
                )
                .unwrap();
                let datagram_object = DatagramObject::ObjectDatagram(datagram_object);

                let _ = object_cache_storage
                    .set_datagram_object(&cache_key, datagram_object, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 2;
        let expected_cache_id = 49;
        let expected_object_payload = vec![14, 15, 16, 17];
        let expected_object = datagram::Object::new(
            track_alias,
            expected_group_id,
            expected_object_id,
            publisher_priority,
            extension_headers.clone(),
            expected_object_payload,
        )
        .unwrap();
        let expected_object = DatagramObject::ObjectDatagram(expected_object);

        let result = object_cache_storage
            .get_latest_datagram_group(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_first_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let extension_headers = vec![];
        let object_status = None;
        let duration = 1000;
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for i in 0..20 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers.clone(),
                object_status,
                object_payload,
            )
            .unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let expected_object_id = 0;
        let expected_cache_id = 0;
        let expected_object_payload = vec![0, 1, 2, 3];
        let expected_object = subgroup_stream::Object::new(
            expected_object_id,
            extension_headers,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_first_subgroup_stream_object(&cache_key, group_id, subgroup_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let extension_headers = vec![];
        let object_status = None;
        let duration = 1000;
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for i in 0..20 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers.clone(),
                object_status,
                object_payload,
            )
            .unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let expected_object_id = 19;
        let expected_cache_id = 19;
        let expected_object_payload = vec![19, 20, 21, 22];
        let expected_object = subgroup_stream::Object::new(
            expected_object_id,
            extension_headers,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_subgroup_stream_object(&cache_key, group_id, subgroup_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_all_subgroup_ids() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 6;
        let extension_headers = vec![];
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        for i in 0..10 {
            let subgroup_id = i as u64;

            let header = subgroup_stream::Header::new(
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap();

            let _ = object_cache_storage
                .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
                .await;

            let subgroup_stream_object = subgroup_stream::Object::new(
                subgroup_id,
                extension_headers.clone(),
                object_status,
                vec![],
            )
            .unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let expected_subgroup_ids: Vec<u64> = (0..10).collect();

        let result = object_cache_storage
            .get_all_subgroup_ids(&cache_key, group_id)
            .await;

        assert!(result.is_ok());

        let subgroup_ids = result.unwrap();
        assert_eq!(subgroup_ids, expected_subgroup_ids);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let extension_headers = vec![];
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for j in 0..4 {
            let group_id = j as u64;
            let group_size = 7;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram_object = datagram::Object::new(
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    extension_headers.clone(),
                    object_payload,
                )
                .unwrap();
                let datagram_object = DatagramObject::ObjectDatagram(datagram_object);

                let _ = object_cache_storage
                    .set_datagram_object(&cache_key, datagram_object, duration)
                    .await;
            }
        }

        let expected_object_id = 6;
        let expected_group_id = 3;

        let group_result = object_cache_storage.get_largest_group_id(&cache_key).await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap().unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage.get_largest_object_id(&cache_key).await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap().unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let extension_headers = vec![];
        let object_status = None;
        let duration = 1000;
        let header = subgroup_stream::Header::new(
            track_alias,
            group_id, // Group ID is fixed
            subgroup_id,
            publisher_priority,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for j in 0..10 {
            let group_size = 15;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let subgroup_stream_object = subgroup_stream::Object::new(
                    object_id,
                    extension_headers.clone(),
                    object_status,
                    object_payload,
                )
                .unwrap();

                let _ = object_cache_storage
                    .set_subgroup_stream_object(
                        &cache_key,
                        group_id,
                        subgroup_id,
                        subgroup_stream_object,
                        duration,
                    )
                    .await;
            }
        }

        let expected_object_id = 14;
        let expected_group_id = 4;

        let group_result = object_cache_storage.get_largest_group_id(&cache_key).await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap().unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage.get_largest_object_id(&cache_key).await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap().unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn delete_client() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let group_id = 4;
        let subgroup_id = 5;
        let track_alias = 3;
        let publisher_priority = 6;
        let header =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header.clone())
            .await;

        let delete_result = object_cache_storage.delete_client(session_id).await;

        assert!(delete_result.is_ok());

        let get_result = object_cache_storage
            .get_subgroup_stream_header(&cache_key, group_id, subgroup_id)
            .await;

        assert!(get_result.is_err());
    }
}
