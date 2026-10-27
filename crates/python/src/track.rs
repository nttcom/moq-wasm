use std::sync::Arc;

use bytes::Bytes;
use moqt::{
    Session, Subscription, TrackObject, TrackReader, object::MalformedTrackError,
    subscribe::SubscribeOption,
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

/// A subscription, iterated for its objects until the publisher ends it.
#[pyclass(module = "moqt")]
pub struct Track {
    session: Arc<Session>,
    subscription: Arc<Subscription>,
    reader: Arc<tokio::sync::Mutex<TrackReader>>,
}

impl Track {
    /// The SUBSCRIBE starts after the largest object. With `current_group`, the
    /// objects of that object's group up to and including it are read first.
    pub(crate) async fn subscribe(
        session: Arc<Session>,
        namespace: String,
        name: String,
        current_group: bool,
    ) -> anyhow::Result<Self> {
        let mut subscriber = session.subscriber();
        let (subscription, reader) = if current_group {
            subscriber.subscribe_current_group(namespace, name).await?
        } else {
            let subscription = subscriber
                .subscribe(namespace, name, SubscribeOption::default())
                .await?;
            let reader = subscriber.track_reader(&subscription);
            (subscription, reader)
        };
        Ok(Self {
            session,
            subscription: Arc::new(subscription),
            reader: Arc::new(tokio::sync::Mutex::new(reader)),
        })
    }
}

#[pymethods]
impl Track {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let reader = self.reader.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut reader = reader.lock().await;
            // A failed subgroup stream does not end the track: a publisher that
            // leaves resets its open group before the track ends, and a GOAWAY's
            // cutover cuts the group still open on the retired connection.
            loop {
                match reader.next_object().await {
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
