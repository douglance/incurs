//! Streaming helpers for the incur framework.
//!
//! Provides utilities for working with async streams of JSON values across
//! CLI, HTTP, and MCP transports. The key pattern is wrapping a stream with
//! a completion signal so that middleware "after" hooks can run after the
//! stream is fully consumed by the transport layer.
//!
//! Ported from streaming patterns in `src/internal/command.ts` and `src/Cli.ts`.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use serde_json::Value;
use tokio::sync::oneshot;

/// A boxed, pinned, `Send` stream of JSON values.
pub type ValueStream = Pin<Box<dyn Stream<Item = Value> + Send>>;

/// Wraps a stream so that a signal is sent when it completes (or is dropped).
///
/// This is used for the middleware + streaming interaction: the middleware
/// chain suspends after calling `next()`, waiting for the stream to be fully
/// consumed. The wrapped stream resolves the oneshot sender in its `Drop`
/// impl (via the stream's final poll or explicit drop), which lets the
/// middleware "after" code run.
///
/// # Arguments
///
/// * `stream` - The inner stream to wrap.
/// * `signal` - A oneshot sender that fires when the stream is done.
pub fn wrap_stream_with_signal(stream: ValueStream, signal: oneshot::Sender<()>) -> ValueStream {
    Box::pin(SignalingStream {
        inner: stream,
        signal: Some(signal),
    })
}

/// A stream wrapper that sends a signal when it completes or is dropped.
struct SignalingStream {
    inner: ValueStream,
    signal: Option<oneshot::Sender<()>>,
}

impl Stream for SignalingStream {
    type Item = Value;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // SAFETY: We only project to `inner` which is already pinned (Pin<Box<...>>).
        let inner = unsafe { self.as_mut().map_unchecked_mut(|s| &mut s.inner) };
        match inner.poll_next(cx) {
            Poll::Ready(None) => {
                // Stream exhausted — fire the signal.
                if let Some(signal) = self.get_mut().signal.take() {
                    let _ = signal.send(());
                }
                Poll::Ready(None)
            }
            other => other,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl Drop for SignalingStream {
    fn drop(&mut self) {
        // If the stream is dropped before completion, still fire the signal
        // so the middleware chain can finish.
        if let Some(signal) = self.signal.take() {
            let _ = signal.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_signal_fires_on_completion() {
        let (tx, rx) = oneshot::channel();
        let inner: ValueStream = Box::pin(futures::stream::iter(vec![
            Value::from(1),
            Value::from(2),
        ]));

        let mut wrapped = wrap_stream_with_signal(inner, tx);

        // Consume the stream.
        let mut items = Vec::new();
        while let Some(item) = wrapped.next().await {
            items.push(item);
        }

        assert_eq!(items, vec![Value::from(1), Value::from(2)]);
        // The signal should have been sent.
        assert!(rx.await.is_ok());
    }

    #[tokio::test]
    async fn test_signal_fires_on_drop() {
        let (tx, rx) = oneshot::channel();
        let inner: ValueStream = Box::pin(futures::stream::iter(vec![
            Value::from(1),
            Value::from(2),
            Value::from(3),
        ]));

        let mut wrapped = wrap_stream_with_signal(inner, tx);

        // Consume only one item, then drop.
        let _ = wrapped.next().await;
        drop(wrapped);

        // The signal should have been sent on drop.
        assert!(rx.await.is_ok());
    }
}
