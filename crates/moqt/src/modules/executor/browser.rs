use std::{future::Future, marker::PhantomData, pin::pin, time::Duration};

use futures::future::{AbortHandle, Abortable};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use super::Elapsed;

pub(crate) struct JoinHandle<T> {
    abort_handle: AbortHandle,
    _output: PhantomData<T>,
}

impl<T> JoinHandle<T> {
    pub(crate) fn abort(&self) {
        self.abort_handle.abort();
    }
}

pub(crate) fn spawn<F>(_name: &str, future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
{
    let (abort_handle, abort_registration) = AbortHandle::new_pair();
    spawn_local(async move {
        let _ = Abortable::new(future, abort_registration).await;
    });
    JoinHandle {
        abort_handle,
        _output: PhantomData,
    }
}

pub(crate) fn try_spawn<F>(name: &str, future: F) -> Option<JoinHandle<F::Output>>
where
    F: Future + 'static,
{
    Some(spawn(name, future))
}

pub(crate) async fn yield_now() {
    let _ = JsFuture::from(js_sys::Promise::resolve(&JsValue::UNDEFINED)).await;
}

pub(crate) async fn timeout<F: Future>(
    duration: Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    let mut future = pin!(future);
    tokio::select! {
        output = &mut future => Ok(output),
        _ = sleep(duration) => Err(Elapsed),
    }
}

async fn sleep(duration: Duration) {
    let millis = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let global = js_sys::global();
        let set_timeout: js_sys::Function =
            js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
                .expect("setTimeout is defined in every browser global scope")
                .unchecked_into();
        set_timeout
            .call2(&global, &resolve, &JsValue::from(millis))
            .expect("setTimeout accepts a function and a delay");
    });
    let _ = JsFuture::from(promise).await;
}
