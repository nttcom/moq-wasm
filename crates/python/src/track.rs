use std::sync::Arc;

use bytes::Bytes;
use moqt::{
    Session, Subscription, TrackObject, TrackReader,
    fetch::{Fetch, FetchDataReceiver, FetchOption},
    object::{MalformedTrackError, ObjectStatus},
    subscribe::{FilterType, SubscribeOption},
};
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*, types::PyBytes};

#[pyclass(frozen, module = "moqt")]
pub struct Object {
    #[pyo3(get)]
    group_id: u64,
    #[pyo3(get)]
    object_id: u64,
    payload: Bytes,
}

#[pymethods]
impl Object {
    #[getter]
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.payload)
    }
}

impl From<TrackObject> for Object {
    fn from(object: TrackObject) -> Self {
        Self {
            group_id: object.group_id,
            object_id: object.object_id,
            payload: object.payload,
        }
    }
}

struct Reading {
    /// The joining FETCH of the group the relay held when the SUBSCRIBE
    /// arrived, read first.
    current_group: Option<FetchDataReceiver>,
    reader: TrackReader,
}

/// A subscription, iterated for its objects until the publisher ends it.
#[pyclass(module = "moqt")]
pub struct Track {
    session: Arc<Session>,
    subscription: Arc<Subscription>,
    reading: Arc<tokio::sync::Mutex<Reading>>,
}

impl Track {
    /// The SUBSCRIBE starts after the largest object. With `current_group`, a
    /// joining FETCH with offset 0 brings the group up to that object, so the
    /// two meet without a gap or a repeat (draft-ietf-moq-transport-14 §9.16.2).
    pub(crate) async fn subscribe(
        session: Arc<Session>,
        namespace: String,
        name: String,
        current_group: bool,
    ) -> anyhow::Result<Self> {
        let option = SubscribeOption {
            filter_type: FilterType::LargestObject,
            ..SubscribeOption::default()
        };
        let mut subscriber = session.subscriber();
        let subscription = subscriber.subscribe(namespace, name, option).await?;
        let reader = subscriber.track_reader(&subscription);
        let current_group = if current_group {
            fetch_current_group(&session, &subscription).await
        } else {
            None
        };
        Ok(Self {
            session,
            subscription: Arc::new(subscription),
            reading: Arc::new(tokio::sync::Mutex::new(Reading {
                current_group,
                reader,
            })),
        })
    }
}

/// `None` when the relay has nothing for the group yet, which the FETCH reports as an error.
async fn fetch_current_group(
    session: &Session,
    subscription: &Subscription,
) -> Option<FetchDataReceiver> {
    let mut subscriber = session.subscriber();
    let fetched = async {
        let handle = subscriber
            .fetch_relative_joining(subscription, 0, FetchOption::default())
            .await?;
        subscriber.accept_fetch_receiver(&handle).await
    };
    match fetched.await {
        Ok(receiver) => Some(receiver),
        Err(err) => {
            tracing::debug!(track = %subscription.track_name(), ?err, "no current group to fetch");
            None
        }
    }
}

#[pymethods]
impl Track {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let reading = self.reading.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut reading = reading.lock().await;
            while let Some(fetch) = reading.current_group.as_mut() {
                match fetch.receive().await {
                    Ok(Fetch::Object(object)) => {
                        if object.status == ObjectStatus::Normal && !object.payload.is_empty() {
                            return Ok(Object {
                                group_id: object.location.group_id,
                                object_id: object.location.object_id,
                                payload: object.payload,
                            });
                        }
                    }
                    Ok(Fetch::UnknownRange { .. }) => {}
                    Ok(Fetch::End) | Err(_) => reading.current_group = None,
                }
            }
            // A failed subgroup stream does not end the track: a publisher that
            // leaves resets its open group before the track ends, and a GOAWAY's
            // cutover cuts the group still open on the retired connection.
            loop {
                match reading.reader.next_object().await {
                    Ok(Some(object)) => return Ok(Object::from(object)),
                    Ok(None) => return Err(PyStopAsyncIteration::new_err("the track ended")),
                    Err(err) if err.is::<MalformedTrackError>() => return Err(err.into()),
                    Err(err) => tracing::debug!(%err, "skipping a failed subgroup stream"),
                }
            }
        })
    }

    /// Sends UNSUBSCRIBE; the iterator ends once the relay stops the track.
    fn unsubscribe<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.session.clone();
        let subscription = self.subscription.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            session.subscriber().unsubscribe(&subscription).await?;
            Ok(())
        })
    }
}
