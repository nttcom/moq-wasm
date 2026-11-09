use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::span_record::entered_span_scope;

thread_local! {
    static TASK_SCOPE: Cell<Option<u64>> = const { Cell::new(None) };
}

pub fn scoped<F: Future>(scope: u64, future: F) -> Scoped<F> {
    Scoped {
        scope,
        future: Box::pin(future),
    }
}

pub struct Scoped<F> {
    scope: u64,
    future: Pin<Box<F>>,
}

impl<F: Future> Future for Scoped<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let scope = self.scope;
        in_scope(scope, || self.future.as_mut().poll(cx))
    }
}

pub fn in_scope<R>(scope: u64, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<u64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TASK_SCOPE.set(self.0);
        }
    }
    let _restore = Restore(TASK_SCOPE.replace(Some(scope)));
    f()
}

/// The scope of the innermost span `SpanRecordLayer` saw entered on this
/// thread, else that of the `Scoped` future or `in_scope` call running;
/// `None` outside both.
pub fn current_scope() -> Option<u64> {
    entered_span_scope().or_else(|| TASK_SCOPE.get())
}

#[cfg(test)]
mod tests {
    use std::task::Waker;

    use super::*;

    fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
        let mut future = Box::pin(future);
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn a_scoped_future_sees_its_scope_only_while_polled() {
        // Act
        let inside = poll_once(scoped(3, async { current_scope() }));

        // Assert
        assert_eq!(inside, Poll::Ready(Some(3)));
        assert_eq!(current_scope(), None);
    }

    #[test]
    fn an_inner_scope_wins_and_the_outer_one_comes_back() {
        // Act
        let (inner, outer) = in_scope(1, || (in_scope(2, current_scope), current_scope()));

        // Assert
        assert_eq!((inner, outer), (Some(2), Some(1)));
    }
}
