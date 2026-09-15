//! The toolkit-neutral interaction model for interactive incurs surfaces.
//!
//! A desktop window and a terminal application need the same things: the list
//! of commands a catalog exposes, a form lowered from each command's input
//! schema, the collected values coerced back to the types the contract
//! declares, a result flattened into labelled rows, and the state of the call
//! in flight. None of that is toolkit-specific, and none of it is here twice.
//!
//! This crate has no user-interface dependency at all. A view owns its own
//! controls and its own event pump; it drives an [`AppSession`] and draws what
//! it reads back.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use incurs_app_model::{AppSession, CallEnvironment, ToolRunner};
//! # fn example(catalog: incurs::tool::ToolCatalog) -> std::io::Result<()> {
//! let runner = Arc::new(ToolRunner::new(catalog, CallEnvironment::Host)?);
//! let mut session = AppSession::new(runner);
//!
//! session.select_command("greet");
//! session.state_mut().value_mut("name").text = "Ada".to_string();
//!
//! if let Ok(handle) = session.start_run() {
//!     // The view pumps `handle.updates` and feeds each one to
//!     // `session.receive(...)` until it observes `RunUpdate::Finished`.
//!     let _ = handle;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! One rule is load-bearing: **the view owns text, the model does not.** A text
//! control is the source of truth for its own buffer, and the view writes it
//! into [`FormState`] immediately before starting a call. A model that also
//! held text would let the two disagree, and a test that set both would pass
//! while proving nothing.

#![deny(missing_docs)]

pub mod form;
pub mod rows;
pub mod runtime;
pub mod session;
pub mod skills;
pub mod text;

pub use form::{FieldIssue, FieldKind, FieldValue, FormField, FormModel, FormState};
pub use rows::{DisplayRow, raw_json, rows_for};
pub use runtime::{CallEnvironment, RunHandle, RunUpdate, ToolRunner};
pub use session::{AppSession, CommandItem, RunState, SkillState};
pub use skills::{SkillPublisher, SkillReport, SkillScope};
pub use text::humanize;
