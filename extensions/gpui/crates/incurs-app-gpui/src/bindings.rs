//! Hooks for building your own GPUI interface over an incurs command graph.
//!
//! [`crate::Workbench`] is a reference application, not the only one. Everything
//! it knows about a command graph comes from [`incurs_app_model`], which has no
//! user-interface dependency; the only genuinely GPUI-specific work is moving
//! results from the call runtime onto the window's executor. That is what this
//! module is, and it is the whole of it.
//!
//! Bring your own components — from `gpui-component`, from a design system, or
//! hand-written — and keep the contract:
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use gpui::Context;
//! # use incurs_app_model::{AppSession, RunUpdate};
//! # use incurs_app_gpui::bindings;
//! # struct MyView { session: AppSession, pump: Option<gpui::Task<()>> }
//! # impl MyView {
//! fn run(&mut self, cx: &mut Context<Self>) {
//!     // Your controls own their text, so write it in before collecting.
//!     // Collect first: reading the controls and writing the session both
//!     // borrow the view.
//!     let texts = self.text_values();
//!     self.session.state_mut().set_texts(texts);
//!
//!     if let Ok(handle) = self.session.start_run() {
//!         self.pump = Some(bindings::pump_run(handle, cx, |view: &mut Self, update, cx| {
//!             view.session.receive(update);
//!             cx.notify();
//!         }));
//!     }
//! }
//! # fn text_values(&self) -> Vec<(String, String)> { Vec::new() }
//! # }
//! ```
//!
//! Three things make an interface an incurs interface, and none of them is a
//! widget: calls go through the tool catalog, values are coerced by
//! [`incurs_app_model::FormState::arguments`] rather than by the view, and
//! field problems come back from the runtime rather than being invented.

use std::sync::Arc;

use futures::StreamExt;
use gpui::{Context, Task};
use incurs_app_model::{RunHandle, RunUpdate, SkillPublisher, SkillReport, ToolRunner};

/// Drives one run's updates onto the window's executor.
///
/// Calls run on a separate runtime, so their updates arrive off the window
/// thread. This forwards each one to `apply` on the foreground executor, in the
/// order the runtime emitted it, and stops when the view goes away — a pump
/// that outlived its view would keep a dead entity alive.
///
/// Exactly one [`RunUpdate::Finished`] arrives, even for a cancelled call, so a
/// view that watches for it never leaves a call visually pending.
///
/// Hold the returned task for as long as the run should continue; dropping it
/// stops the pump.
pub fn pump_run<V: 'static>(
    handle: RunHandle,
    cx: &mut Context<V>,
    mut apply: impl FnMut(&mut V, RunUpdate, &mut Context<V>) + 'static,
) -> Task<()> {
    let mut updates = handle.updates;
    cx.spawn(async move |view, cx| {
        while let Some(update) = updates.next().await {
            if view.update(cx, |view, cx| apply(view, update, cx)).is_err() {
                break;
            }
        }
    })
}

/// Installs agent skills off the window thread.
///
/// Generating and writing skill files touches the filesystem, so it runs on the
/// call runtime and reports back on the foreground executor. `apply` receives
/// the report, or the reason it failed.
///
/// Hold the returned task until it completes; dropping it abandons the report,
/// though the installation itself will already have run.
pub fn install_skills<V: 'static>(
    runner: &Arc<ToolRunner>,
    publisher: SkillPublisher,
    cx: &mut Context<V>,
    apply: impl FnOnce(&mut V, Result<SkillReport, String>, &mut Context<V>) + 'static,
) -> Task<()> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    runner.spawn(async move {
        let _ = sender.send(match publisher.install().await {
            Ok(result) => Ok(SkillReport::from_result(&result)),
            Err(error) => Err(error.to_string()),
        });
    });

    cx.spawn(async move |view, cx| {
        let outcome = receiver.await;
        let _ = view.update(cx, |view, cx| {
            apply(
                view,
                match outcome {
                    Ok(result) => result,
                    Err(_) => Err("Installation stopped unexpectedly.".to_string()),
                },
                cx,
            );
        });
    })
}
