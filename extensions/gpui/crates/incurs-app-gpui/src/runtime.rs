//! Bridge between the GPUI foreground loop and the incurs tool runtime.
//!
//! GPUI drives its own foreground executor on the main thread, while incurs
//! commands are ordinary Tokio futures. A [`ToolRunner`] owns a multi-threaded
//! Tokio runtime, dispatches one call onto it, and reports ordered updates back
//! through a channel the UI can poll from a foreground task.
//!
//! The runner adds no policy of its own. Cancellation, environment resolution,
//! config defaults, middleware, and validation all remain the shared command
//! runtime's behavior, reached through [`ToolCatalog::call`].

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::channel::mpsc;
use incurs::tool::{
    ConfigSource, EnvironmentSource, ToolCallControl, ToolCallOptions, ToolCallOutcome,
    ToolCatalog, ToolEvent, ToolEventSink,
};
use serde_json::Value;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

/// One ordered update from a running tool call.
#[derive(Debug)]
pub enum RunUpdate {
    /// An incremental event emitted while the command runs.
    Event(ToolEvent),
    /// The terminal outcome. No further updates follow.
    Finished(Box<ToolCallOutcome>),
}

/// How a desktop call resolves environment values and config defaults.
///
/// A shipped application normally behaves like the CLI it was built from, so
/// [`CallEnvironment::Host`] is the default. [`CallEnvironment::Isolated`]
/// suits a kiosk build that must not read the user's shell environment or
/// configuration files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CallEnvironment {
    /// Read declared environment fields and discovered config files.
    #[default]
    Host,
    /// Supply neither environment values nor config defaults.
    Isolated,
}

impl CallEnvironment {
    /// Builds call options carrying this environment policy and control.
    fn options(self, control: ToolCallControl) -> ToolCallOptions {
        match self {
            CallEnvironment::Host => ToolCallOptions {
                environment: EnvironmentSource::DeclaredHost,
                config: ConfigSource::Auto,
                control,
                ..ToolCallOptions::default()
            },
            CallEnvironment::Isolated => ToolCallOptions {
                control,
                ..ToolCallOptions::isolated()
            },
        }
    }
}

/// A running call's update stream and cancellation signal.
pub struct RunHandle {
    /// Ordered updates, ending with exactly one [`RunUpdate::Finished`].
    pub updates: mpsc::UnboundedReceiver<RunUpdate>,
    /// Cooperative cancellation signal for the running command.
    pub cancellation: CancellationToken,
}

impl RunHandle {
    /// Requests cooperative cancellation of the running command.
    ///
    /// The command still reports a terminal outcome, so the UI always observes
    /// a [`RunUpdate::Finished`] and never leaves a call visually pending.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

/// Dispatches tool calls onto a Tokio runtime owned by the application.
pub struct ToolRunner {
    catalog: ToolCatalog,
    environment: CallEnvironment,
    runtime: Arc<Runtime>,
}

impl ToolRunner {
    /// Creates a runner with its own multi-threaded Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns the Tokio error when the runtime cannot be created.
    pub fn new(catalog: ToolCatalog, environment: CallEnvironment) -> std::io::Result<Self> {
        Ok(Self {
            catalog,
            environment,
            runtime: Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .thread_name("incurs-app")
                    .build()?,
            ),
        })
    }

    /// Returns the catalog the runner dispatches into.
    pub fn catalog(&self) -> &ToolCatalog {
        &self.catalog
    }

    /// Runs one future on the call runtime.
    ///
    /// The window's own executor is single threaded and drives drawing, so
    /// filesystem and network work belongs here instead.
    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future);
    }

    /// Starts one tool call and returns its update stream.
    ///
    /// The call runs on the Tokio runtime. Events are forwarded in the order
    /// the runtime emits them, followed by exactly one terminal outcome.
    pub fn start(&self, name: &str, arguments: BTreeMap<String, Value>) -> RunHandle {
        let (sender, updates) = mpsc::unbounded();
        let cancellation = CancellationToken::new();

        let control = ToolCallControl {
            cancellation: cancellation.clone(),
            events: Some(Arc::new(ChannelEventSink {
                sender: sender.clone(),
            })),
        };
        let options = self.environment.options(control);

        let catalog = self.catalog.clone();
        let name = name.to_string();
        self.runtime.spawn(async move {
            let outcome = catalog.call(&name, arguments, options).await;
            let _ = sender.unbounded_send(RunUpdate::Finished(Box::new(outcome)));
        });

        RunHandle {
            updates,
            cancellation,
        }
    }
}

/// Forwards runtime events onto the UI update channel.
struct ChannelEventSink {
    sender: mpsc::UnboundedSender<RunUpdate>,
}

#[async_trait::async_trait]
impl ToolEventSink for ChannelEventSink {
    async fn emit(&self, event: ToolEvent) {
        let _ = self.sender.unbounded_send(RunUpdate::Event(event));
    }
}
