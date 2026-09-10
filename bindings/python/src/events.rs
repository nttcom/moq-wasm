use std::sync::Mutex;

use anyhow::Context;
use moqt::{
    ContentExists, DUAL, FilterType, PublishHandler, PublishNamespaceHandler, SubscribeHandler,
    SubscribeNamespaceHandler,
};
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;

use crate::{
    session::SharedSession,
    track_reader::TrackReader,
    track_writer::{TrackWriter, first_group_id_or_now},
};

const SUBSCRIBER_PRIORITY: u8 = 128;
const NEVER_EXPIRES: u64 = 0;

fn take_handler<H>(handler: &Mutex<Option<H>>) -> anyhow::Result<H> {
    handler
        .lock()
        .expect("handler mutex is never poisoned: no code panics while holding it")
        .take()
        .context("this request has already been accepted or rejected")
}

/// A peer's SUBSCRIBE for a track this side publishes. `accept()` answers
/// SUBSCRIBE_OK and returns the `TrackWriter` for the track.
#[pyclass(frozen)]
pub(crate) struct SubscribeRequest {
    session: SharedSession,
    #[pyo3(get)]
    namespace: String,
    #[pyo3(get)]
    name: String,
    handler: Mutex<Option<SubscribeHandler<DUAL>>>,
}

impl SubscribeRequest {
    pub(crate) fn new(session: SharedSession, handler: SubscribeHandler<DUAL>) -> Self {
        Self {
            session,
            namespace: handler.track_namespace.clone(),
            name: handler.track_name.clone(),
            handler: Mutex::new(Some(handler)),
        }
    }
}

#[pymethods]
impl SubscribeRequest {
    #[pyo3(signature = (*, first_group_id = None))]
    fn accept<'py>(
        &self,
        py: Python<'py>,
        first_group_id: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        let session = self.session.clone();
        future_into_py(py, async move {
            let track_alias = handler
                .ok(NEVER_EXPIRES, ContentExists::False)
                .await
                .map_err(anyhow::Error::from)?;
            let subscription = handler.into_subscription(track_alias);
            let factory = session.publisher().create_stream(&subscription);
            Ok(TrackWriter::new(moqt::TrackWriter::new(
                factory,
                first_group_id_or_now(first_group_id),
            )))
        })
    }

    fn reject<'py>(
        &self,
        py: Python<'py>,
        code: u64,
        reason: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        future_into_py(py, async move {
            handler
                .error(code, reason)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(())
        })
    }
}

/// A peer's PUBLISH offering a track to this side. `accept()` answers
/// PUBLISH_OK and returns the `TrackReader` for the track.
#[pyclass(frozen)]
pub(crate) struct PublishRequest {
    session: SharedSession,
    #[pyo3(get)]
    namespace: String,
    #[pyo3(get)]
    name: String,
    handler: Mutex<Option<PublishHandler<DUAL>>>,
}

impl PublishRequest {
    pub(crate) fn new(session: SharedSession, handler: PublishHandler<DUAL>) -> Self {
        Self {
            session,
            namespace: handler.track_namespace.clone(),
            name: handler.track_name.clone(),
            handler: Mutex::new(Some(handler)),
        }
    }
}

#[pymethods]
impl PublishRequest {
    fn accept<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        let session = self.session.clone();
        future_into_py(py, async move {
            let subscription = handler
                .ok(
                    SUBSCRIBER_PRIORITY,
                    FilterType::LargestObject,
                    NEVER_EXPIRES,
                )
                .await
                .map_err(anyhow::Error::from)?;
            handler.accept_data_receiver().await;
            Ok(TrackReader::pending(session, subscription))
        })
    }

    fn reject<'py>(
        &self,
        py: Python<'py>,
        code: u64,
        reason: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        future_into_py(py, async move {
            handler
                .error(code, reason)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(())
        })
    }
}

#[pyclass(frozen)]
pub(crate) struct PublishNamespaceRequest {
    #[pyo3(get)]
    namespace: String,
    handler: Mutex<Option<PublishNamespaceHandler<DUAL>>>,
}

impl PublishNamespaceRequest {
    pub(crate) fn new(handler: PublishNamespaceHandler<DUAL>) -> Self {
        Self {
            namespace: handler.track_namespace.clone(),
            handler: Mutex::new(Some(handler)),
        }
    }
}

#[pymethods]
impl PublishNamespaceRequest {
    fn accept<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        future_into_py(py, async move {
            handler.ok().await.map_err(anyhow::Error::from)?;
            Ok(())
        })
    }

    fn reject<'py>(
        &self,
        py: Python<'py>,
        code: u64,
        reason: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        future_into_py(py, async move {
            handler
                .error(code, reason)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(())
        })
    }
}

#[pyclass(frozen)]
pub(crate) struct SubscribeNamespaceRequest {
    #[pyo3(get)]
    prefix: String,
    handler: Mutex<Option<SubscribeNamespaceHandler<DUAL>>>,
}

impl SubscribeNamespaceRequest {
    pub(crate) fn new(handler: SubscribeNamespaceHandler<DUAL>) -> Self {
        Self {
            prefix: handler.track_namespace_prefix.clone(),
            handler: Mutex::new(Some(handler)),
        }
    }
}

#[pymethods]
impl SubscribeNamespaceRequest {
    fn accept<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        future_into_py(py, async move {
            handler.ok().await.map_err(anyhow::Error::from)?;
            Ok(())
        })
    }

    fn reject<'py>(
        &self,
        py: Python<'py>,
        code: u64,
        reason: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let handler = take_handler(&self.handler)?;
        future_into_py(py, async move {
            handler
                .error(code, reason)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(())
        })
    }
}

#[pyclass(frozen)]
pub(crate) struct Disconnected;

#[pyclass(frozen)]
pub(crate) struct ProtocolViolation;
