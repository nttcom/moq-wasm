use super::commands::ObjectCacheStorageCommand;
use crate::modules::object_cache_storage::cache::{
    Cache, CacheKey, datagram::DatagramCache, subgroup_stream::SubgroupStreamsCache,
};
use std::{collections::HashMap, time::Duration};
use tokio::sync::mpsc;

pub(crate) async fn object_cache_storage(rx: &mut mpsc::Receiver<ObjectCacheStorageCommand>) {
    tracing::trace!("object_cache_storage start");

    // TODO: set accurate size
    let max_cache_size = 100000;

    let mut storage = HashMap::<CacheKey, Cache>::new();
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    interval.tick().await;

    loop {
        tokio::select! {
            // purgeよりもコマンド処理を優先する
            biased;
            Some(cmd) = rx.recv() => {
                tracing::trace!("command received: {:#?}", cmd);
                match cmd {
                    ObjectCacheStorageCommand::CreateDatagramCache { cache_key, resp } => {
                        let datagram_cache = DatagramCache::new(max_cache_size);
                        let cache = Cache::Datagram(datagram_cache);

                        // Insert the DatagramCache into the ObjectCacheStorage
                        storage.insert(cache_key.clone(), cache);

                        resp.send(Ok(())).unwrap();
                    }
                    ObjectCacheStorageCommand::CreateSubgroupStreamCache {
                        cache_key,
                        group_id,
                        subgroup_id,
                        header,
                        resp,
                    } => {
                        // If the SubgroupStreamCache does not exist, create a new cache
                        let cache = storage
                            .entry(cache_key)
                            .or_insert_with(|| Cache::SubgroupStream(SubgroupStreamsCache::new()));

                        let subgroup_stream_cache = match cache {
                            Cache::SubgroupStream(subgroup_stream_cache) => subgroup_stream_cache,
                            _ => unreachable!(),
                        };

                        // Add a new SubgroupStream to the SubgroupCache
                        subgroup_stream_cache.set_subgroup_stream(
                            group_id,
                            subgroup_id,
                            header,
                            max_cache_size,
                        );

                        resp.send(Ok(())).unwrap();
                    }
                    ObjectCacheStorageCommand::HasDatagramCache { cache_key, resp } => {
                        let cache = storage.get(&cache_key);
                        match cache {
                            Some(Cache::Datagram(_)) => {
                                resp.send(Ok(true)).unwrap();
                            }
                            _ => {
                                resp.send(Ok(false)).unwrap();
                            }
                        }
                    }
                    ObjectCacheStorageCommand::GetSubgroupStreamHeader {
                        cache_key,
                        group_id,
                        subgroup_id,
                        resp,
                    } => {
                        let cache = storage.get(&cache_key);
                        let subgroup_stream_cache = match cache {
                            Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let header = subgroup_stream_cache.get_header(group_id, subgroup_id);
                        resp.send(Ok(header)).unwrap();
                    }
                    ObjectCacheStorageCommand::SetDatagramObject {
                        cache_key,
                        datagram_object,
                        duration,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let datagram_cache = match cache {
                            Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        datagram_cache.insert_object(datagram_object, duration);
                        resp.send(Ok(())).unwrap();
                    }
                    ObjectCacheStorageCommand::SetSubgroupStreamObject {
                        cache_key,
                        group_id,
                        subgroup_id,
                        subgroup_stream_object,
                        duration,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let subgroup_streams_cache = match cache {
                            Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        subgroup_streams_cache.insert_object(
                            group_id,
                            subgroup_id,
                            subgroup_stream_object,
                            duration,
                        );
                        resp.send(Ok(())).unwrap();
                    }
                    ObjectCacheStorageCommand::GetDatagramObject {
                        cache_key,
                        group_id,
                        object_id,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let datagram_cache = match cache {
                            Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id = datagram_cache.get_object(group_id, object_id);
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetAbsoluteOrNextSubgroupStreamObject {
                        cache_key,
                        group_id,
                        subgroup_id,
                        object_id,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let subgroup_streams_cache = match cache {
                            Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id = subgroup_streams_cache.get_absolute_or_next_object(
                            group_id,
                            subgroup_id,
                            object_id,
                        );
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetNextDatagramObject {
                        cache_key,
                        cache_id,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let datagram_cache = match cache {
                            Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id = datagram_cache.get_next_object(cache_id);
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetNextSubgroupStreamObject {
                        cache_key,
                        group_id,
                        subgroup_id,
                        cache_id,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let subgroup_streams_cache = match cache {
                            Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id =
                            subgroup_streams_cache.get_next_object(group_id, subgroup_id, cache_id);
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetLatestDatagramGroup { cache_key, resp } => {
                        let cache = storage.get_mut(&cache_key);
                        let datagram_cache = match cache {
                            Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id = datagram_cache.get_latest_group();
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetLatestDatagramObject { cache_key, resp } => {
                        let cache = storage.get_mut(&cache_key);
                        let datagram_cache = match cache {
                            Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id = datagram_cache.get_latest_object();
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetFirstSubgroupStreamObject {
                        cache_key,
                        group_id,
                        subgroup_id,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let subgroup_streams_cache = match cache {
                            Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id =
                            subgroup_streams_cache.get_first_object(group_id, subgroup_id);
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetLatestSubgroupStreamObject {
                        cache_key,
                        group_id,
                        subgroup_id,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let subgroup_streams_cache = match cache {
                            Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let object_with_cache_id =
                            subgroup_streams_cache.get_latest_object(group_id, subgroup_id);
                        resp.send(Ok(object_with_cache_id)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetAllSubgroupIds {
                        cache_key,
                        group_id,
                        resp,
                    } => {
                        let cache = storage.get_mut(&cache_key);
                        let subgroup_streams_cache = match cache {
                            Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                            _ => {
                                resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                                    .unwrap();
                                continue;
                            }
                        };

                        let subgroup_ids = subgroup_streams_cache.get_all_subgroup_ids(group_id);
                        resp.send(Ok(subgroup_ids)).unwrap();
                    }
                    ObjectCacheStorageCommand::GetLargestGroupId { cache_key, resp } => {
                        let cache = storage.get_mut(&cache_key);
                        if let Some(cache) = cache {
                            let largest_group_id = match cache {
                                Cache::Datagram(datagram_cache) => datagram_cache.get_largest_group_id(),
                                Cache::SubgroupStream(subgroup_stream_cache) => {
                                    subgroup_stream_cache.get_largest_group_id()
                                }
                            };

                            resp.send(Ok(largest_group_id)).unwrap();
                        } else {
                            resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                        }
                    }
                    ObjectCacheStorageCommand::GetLargestObjectId { cache_key, resp } => {
                        let cache = storage.get_mut(&cache_key);
                        if let Some(cache) = cache {
                            let largest_object_id = match cache {
                                Cache::Datagram(datagram_cache) => datagram_cache.get_largest_object_id(),
                                Cache::SubgroupStream(subgroup_stream_cache) => {
                                    subgroup_stream_cache.get_largest_object_id()
                                }
                            };

                            resp.send(Ok(largest_object_id)).unwrap();
                        } else {
                            resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                        }
                    }
                    ObjectCacheStorageCommand::DeleteClient { session_id, resp } => {
                        let keys: Vec<CacheKey> = storage.keys().cloned().collect();
                        for key in keys {
                            if key.session_id() == session_id {
                                let _ = storage.remove(&key);
                            }
                        }
                        resp.send(Ok(())).unwrap();
                    }
                }
            }
            _ = interval.tick() => {
                tracing::trace!("interval ticked");
                for (_, cache) in storage.iter_mut() {
                    match cache {
                        Cache::Datagram(_datagram_cache) => {
                            // datagram_cache.purge();
                        }
                        Cache::SubgroupStream(subgroup_stream_cache) => {
                            subgroup_stream_cache.purge_subgroup_streams();
                        }
                    }
                }
            }
        }
    }
}
