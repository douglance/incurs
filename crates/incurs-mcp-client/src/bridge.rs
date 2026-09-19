//! Moving downstream I/O off the Code Mode executor thread.

use std::future::Future;

/// A handle to a runtime that owns every downstream MCP connection.
///
/// Code Mode's local executor runs its QuickJS pass on a current-thread runtime
/// built with the time driver only. Constructing or polling a child process or a
/// socket there panics with "there is no reactor running", so every operation
/// that touches a transport is moved onto this handle and its outcome returned
/// through a oneshot channel. Awaiting that channel needs only the waker, which
/// the executor thread does have.
///
/// Enabling the I/O driver on the executor thread instead would work, and would
/// be worse: downstream process reaping and socket polling would then sit behind
/// whatever the QuickJS pass is doing.
#[derive(Clone, Debug)]
pub struct IoBridge {
    handle: tokio::runtime::Handle,
}

/// A downstream operation could not be run to completion.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The bridged runtime shut down before the operation finished.
    #[error("the MCP runtime stopped before the call completed")]
    RuntimeStopped,
}

impl IoBridge {
    /// Captures the currently running runtime.
    ///
    /// Must be called from the runtime that should own downstream I/O, and
    /// therefore before Code Mode's executor thread is spawned: the factory
    /// closure passed to that thread runs on the new thread, not this one.
    ///
    /// # Panics
    /// Panics when called outside a Tokio runtime.
    #[must_use]
    pub fn current() -> Self {
        Self {
            handle: tokio::runtime::Handle::current(),
        }
    }

    /// Bridges to an explicit runtime handle.
    #[must_use]
    pub fn new(handle: tokio::runtime::Handle) -> Self {
        Self { handle }
    }

    /// Returns the bridged runtime handle.
    #[must_use]
    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    /// Runs one I/O-bearing future on the bridged runtime and awaits its value.
    ///
    /// # Errors
    /// Returns [`BridgeError::RuntimeStopped`] if the runtime is shut down
    /// before the future completes.
    pub async fn run<T, F>(&self, future: F) -> Result<T, BridgeError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.handle.spawn(async move {
            let _ = sender.send(future.await);
        });
        receiver.await.map_err(|_| BridgeError::RuntimeStopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_a_future_on_the_bridged_runtime() {
        let bridge = IoBridge::current();
        let value = bridge.run(async { 7_u32 }).await.expect("bridged run");
        assert_eq!(value, 7);
    }

    #[test]
    fn a_thread_without_an_io_driver_can_still_await_through_the_bridge() {
        // This mirrors the executor thread exactly: current-thread, time only.
        // Doing the work here directly would panic; through the bridge it does
        // not, which is the entire reason the bridge exists.
        let owner = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("owner runtime");
        let bridge = IoBridge::new(owner.handle().clone());

        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("executor runtime");
        let value = executor.block_on(async {
            bridge
                .run(async {
                    // A real transport would bind or spawn here.
                    tokio::net::TcpListener::bind("127.0.0.1:0")
                        .await
                        .map(|listener| listener.local_addr().is_ok())
                        .unwrap_or(false)
                })
                .await
                .expect("bridged run")
        });
        assert!(value, "the bridged runtime must own the I/O driver");
    }
}
