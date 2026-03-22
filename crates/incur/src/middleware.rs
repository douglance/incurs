//! Middleware types and composition for the incur framework.
//!
//! Middleware wraps command execution in an onion-style chain: each handler
//! receives a context and a `next` function. Calling `next` runs the inner
//! layers (and eventually the command handler). Code before `next()` runs
//! "before" the command; code after runs "after".
//!
//! Ported from `src/middleware.ts`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::output::Format;

/// A boxed, pinned, `Send` future. Used as the return type for middleware
/// and composition helpers where we need type-erased async.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A middleware handler function.
///
/// Takes a [`MiddlewareContext`] and a [`MiddlewareNext`] callback.
/// The handler must call `next` exactly once to proceed to the inner handler
/// (unless it wants to short-circuit). It returns a `BoxFuture<()>`.
pub type MiddlewareFn =
    Arc<dyn Fn(MiddlewareContext, MiddlewareNext) -> BoxFuture<()> + Send + Sync>;

/// The "next" callback passed to middleware. Calling it invokes the next
/// middleware in the chain (or the command handler at the innermost layer).
///
/// This is `FnOnce` because it can only be called once per invocation.
pub type MiddlewareNext = Box<dyn FnOnce() -> BoxFuture<()> + Send>;

/// Context available inside middleware.
///
/// Provides read access to request metadata and read-write access to shared
/// variables (via `vars`). Middleware can set variables that downstream
/// middleware and the command handler can read.
pub struct MiddlewareContext {
    /// Whether the consumer is an agent (stdout is not a TTY).
    pub agent: bool,
    /// The resolved command path (e.g. `"users list"`).
    pub command: String,
    /// Parsed environment variables from the CLI-level env schema.
    pub env: Value,
    /// The resolved output format.
    pub format: Format,
    /// Whether the user explicitly passed `--format` or `--json`.
    pub format_explicit: bool,
    /// The CLI name.
    pub name: String,
    /// Shared variables set by upstream middleware. Downstream middleware and
    /// the command handler read from this map. Use the `RwLock` to set values.
    pub vars: Arc<RwLock<serde_json::Map<String, Value>>>,
    /// The CLI version string.
    pub version: Option<String>,
}

impl Clone for MiddlewareContext {
    fn clone(&self) -> Self {
        MiddlewareContext {
            agent: self.agent,
            command: self.command.clone(),
            env: self.env.clone(),
            format: self.format,
            format_explicit: self.format_explicit,
            name: self.name.clone(),
            vars: Arc::clone(&self.vars),
            version: self.version.clone(),
        }
    }
}

/// Composes a slice of middleware into an onion-style chain.
///
/// Middleware is composed right-to-left (the first middleware in the slice
/// is the outermost layer). The `final_handler` is the innermost function
/// that actually runs the command.
///
/// # Example (conceptual)
///
/// Given middleware `[A, B]` and a final handler `H`, the execution order is:
/// ```text
/// A before → B before → H → B after → A after
/// ```
pub fn compose(
    middlewares: &[MiddlewareFn],
    ctx: MiddlewareContext,
    final_handler: impl FnOnce() -> BoxFuture<()> + Send + 'static,
) -> BoxFuture<()> {
    // Build the chain from right to left (reduceRight in TypeScript).
    // Start with the final handler as the innermost "next".
    let mut next: Box<dyn FnOnce() -> BoxFuture<()> + Send> = Box::new(final_handler);

    for mw in middlewares.iter().rev() {
        let mw = Arc::clone(mw);
        let ctx = ctx.clone();
        let current_next = next;
        next = Box::new(move || -> BoxFuture<()> {
            Box::pin(async move {
                mw(ctx, current_next).await;
            })
        });
    }

    next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_ctx() -> MiddlewareContext {
        MiddlewareContext {
            agent: false,
            command: "test".to_string(),
            env: Value::Null,
            format: Format::Toon,
            format_explicit: false,
            name: "test-cli".to_string(),
            vars: Arc::new(RwLock::new(serde_json::Map::new())),
            version: None,
        }
    }

    #[tokio::test]
    async fn test_compose_empty_middleware() {
        let called = Arc::new(AtomicUsize::new(0));
        let called_clone = Arc::clone(&called);

        let future = compose(&[], make_ctx(), move || {
            Box::pin(async move {
                called_clone.fetch_add(1, Ordering::SeqCst);
            })
        });

        future.await;
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_compose_onion_order() {
        let order = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));

        let order_a = Arc::clone(&order);
        let mw_a: MiddlewareFn = Arc::new(move |_ctx, next| {
            let order = Arc::clone(&order_a);
            Box::pin(async move {
                order.lock().await.push("A-before".to_string());
                next().await;
                order.lock().await.push("A-after".to_string());
            })
        });

        let order_b = Arc::clone(&order);
        let mw_b: MiddlewareFn = Arc::new(move |_ctx, next| {
            let order = Arc::clone(&order_b);
            Box::pin(async move {
                order.lock().await.push("B-before".to_string());
                next().await;
                order.lock().await.push("B-after".to_string());
            })
        });

        let order_final = Arc::clone(&order);
        let future = compose(&[mw_a, mw_b], make_ctx(), move || {
            Box::pin(async move {
                order_final.lock().await.push("handler".to_string());
            })
        });

        future.await;

        let result = order.lock().await;
        assert_eq!(
            *result,
            vec!["A-before", "B-before", "handler", "B-after", "A-after"]
        );
    }
}
