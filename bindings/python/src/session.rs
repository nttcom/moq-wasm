use std::sync::Arc;

use moqt::{
    DUAL, FilterType, GroupOrder, PublishOption, SessionEvent, SubscribeOption,
    TerminationErrorCode,
};
use pyo3::{exceptions::PyStopAsyncIteration, prelude::*};
use pyo3_async_runtimes::tokio::future_into_py;

use crate::{
    events::{
        Disconnected, ProtocolViolation, PublishNamespaceRequest, PublishRequest,
        SubscribeNamespaceRequest, SubscribeRequest,
    },
    track_reader::TrackReader,
    track_writer::{TrackWriter, first_group_id_or_now},
};

const SUBSCRIBER_PRIORITY: u8 = 128;

pub(crate) type SharedSession = Arc<moqt::Session<DUAL>>;

#[pyclass(frozen)]
pub(crate) struct Session {
    inner: SharedSession,
}

impl Session {
    pub(crate) fn new(session: moqt::Session<DUAL>) -> Self {
        Self {
            inner: Arc::new(session),
        }
    }
}

#[pymethods]
impl Session {
    fn subscribe<'py>(
        &self,
        py: Python<'py>,
        namespace: String,
        name: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        future_into_py(py, async move {
            let option = SubscribeOption {
                subscriber_priority: SUBSCRIBER_PRIORITY,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::NextGroupStart,
            };
            let subscription = session
                .subscriber()
                .subscribe(namespace, name, option)
                .await?;
            Ok(TrackReader::pending(session, subscription))
        })
    }

    #[pyo3(signature = (namespace, name, *, first_group_id = None))]
    fn publish<'py>(
        &self,
        py: Python<'py>,
        namespace: String,
        name: String,
        first_group_id: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        future_into_py(py, async move {
            let publisher = session.publisher();
            let subscription = publisher
                .publish(namespace, name, PublishOption::default())
                .await?;
            let factory = publisher.create_stream(&subscription);
            Ok(TrackWriter::new(moqt::TrackWriter::new(
                factory,
                first_group_id_or_now(first_group_id),
            )))
        })
    }

    fn publish_namespace<'py>(
        &self,
        py: Python<'py>,
        namespace: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        future_into_py(py, async move {
            session.publisher().publish_namespace(namespace).await?;
            Ok(())
        })
    }

    fn subscribe_namespace<'py>(
        &self,
        py: Python<'py>,
        prefix: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        future_into_py(py, async move {
            session.subscriber().subscribe_namespace(prefix).await?;
            Ok(())
        })
    }

    /// Resolves to the next inbound request or state change, or `None` once
    /// the session has ended. Requests not exposed to Python (FETCH,
    /// TRACK_STATUS, SUBSCRIBE_UPDATE, ...) are skipped; their unanswered
    /// handlers reply with an error when dropped.
    fn next_event<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        future_into_py(py, async move {
            let event = next_python_event(session).await;
            Python::attach(|py| event.into_pyobject(py).map(Bound::unbind))
        })
    }

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        future_into_py(py, async move {
            match next_python_event(session).await {
                Some(event) => Python::attach(|py| event.into_pyobject(py).map(Bound::unbind)),
                None => Err(PyStopAsyncIteration::new_err(())),
            }
        })
    }

    #[pyo3(signature = (reason = ""))]
    fn close(&self, reason: &str) {
        self.inner
            .close_with_error(TerminationErrorCode::NoError, reason);
    }
}

enum PythonEvent {
    Subscribe(SubscribeRequest),
    Publish(PublishRequest),
    PublishNamespace(PublishNamespaceRequest),
    SubscribeNamespace(SubscribeNamespaceRequest),
    Disconnected,
    ProtocolViolation,
}

impl<'py> IntoPyObject<'py> for PythonEvent {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        Ok(match self {
            Self::Subscribe(request) => request.into_pyobject(py)?.into_any(),
            Self::Publish(request) => request.into_pyobject(py)?.into_any(),
            Self::PublishNamespace(request) => request.into_pyobject(py)?.into_any(),
            Self::SubscribeNamespace(request) => request.into_pyobject(py)?.into_any(),
            Self::Disconnected => Disconnected.into_pyobject(py)?.into_any(),
            Self::ProtocolViolation => ProtocolViolation.into_pyobject(py)?.into_any(),
        })
    }
}

async fn next_python_event(session: SharedSession) -> Option<PythonEvent> {
    loop {
        let event = match session.receive_event().await {
            Ok(event) => event,
            Err(_) => return None,
        };
        let python_event = match event {
            SessionEvent::Subscribe(handler) => {
                PythonEvent::Subscribe(SubscribeRequest::new(session.clone(), handler))
            }
            SessionEvent::Publish(handler) => {
                PythonEvent::Publish(PublishRequest::new(session.clone(), handler))
            }
            SessionEvent::PublishNamespace(handler) => {
                PythonEvent::PublishNamespace(PublishNamespaceRequest::new(handler))
            }
            SessionEvent::SubscribeNameSpace(handler) => {
                PythonEvent::SubscribeNamespace(SubscribeNamespaceRequest::new(handler))
            }
            SessionEvent::Disconnected() => PythonEvent::Disconnected,
            SessionEvent::ProtocolViolation() => PythonEvent::ProtocolViolation,
            SessionEvent::GoAway(_)
            | SessionEvent::MaxRequestId(_)
            | SessionEvent::RequestsBlocked(_)
            | SessionEvent::PublishNamespaceDone(_)
            | SessionEvent::PublishNamespaceCancel(_)
            | SessionEvent::UnsubscribeNamespace(_)
            | SessionEvent::PublishDone(_)
            | SessionEvent::SubscribeUpdate(_)
            | SessionEvent::Unsubscribe(_)
            | SessionEvent::Fetch(_)
            | SessionEvent::FetchCancel(_)
            | SessionEvent::TrackStatus(_) => continue,
        };
        return Some(python_event);
    }
}
