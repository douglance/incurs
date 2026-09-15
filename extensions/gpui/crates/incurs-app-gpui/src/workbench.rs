//! The desktop workbench view.
//!
//! The workbench is a Tool Binding: it exposes the Tool Contracts of one
//! [`ToolCatalog`] to a person. Commands are listed on the left, the selected
//! command's inputs are collected in the middle, and its result is shown
//! below. Every call goes through the shared command runtime, so validation,
//! middleware, config defaults, and cancellation behave exactly as they do on
//! the CLI.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::StreamExt;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render, SharedString,
    Styled, Task, Window, div, prelude::*, px,
};
use incurs::output::{CtaBlock, CtaEntry};
use serde_json::Value;

use incurs_app_model::form::{FieldKind, FormModel};
use incurs_app_model::rows::{DisplayRow, raw_json, rows_for};
use incurs_app_model::session::{AppSession, RunState, SkillState};
use incurs_app_model::{RunUpdate, SkillPublisher, SkillReport, ToolRunner};

use crate::text_field::{TextField, TextFieldEvent};
use crate::theme::{FIELD_HEIGHT, RADIUS, SIDEBAR_WIDTH, Theme};

/// The root view of the desktop application.
pub struct Workbench {
    /// Everything that is not specific to this toolkit.
    session: AppSession,
    app_title: SharedString,
    /// This view owns its text buffers; the session does not. The controls are
    /// the source of truth, written into form state just before a call.
    search: Entity<TextField>,
    fields: BTreeMap<String, Entity<TextField>>,
    /// Whether the raw-JSON disclosure is open. Presentation, not domain state.
    raw_open: bool,
    run_task: Option<Task<()>>,
    skill_task: Option<Task<()>>,
    focus_handle: FocusHandle,
}

impl Workbench {
    /// Creates the workbench for one runner's catalog.
    pub fn new(
        runner: Arc<ToolRunner>,
        app_title: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.set_global(Theme::for_appearance(window.appearance()));

        let search = cx.new(|cx| TextField::new("Search commands", cx));
        cx.subscribe(&search, |this: &mut Self, _, _: &TextFieldEvent, cx| {
            cx.notify();
            let _ = this;
        })
        .detach();

        let mut workbench = Self {
            session: AppSession::new(runner),
            app_title,
            search,
            fields: BTreeMap::new(),
            raw_open: false,
            run_task: None,
            skill_task: None,
            focus_handle: cx.focus_handle(),
        };
        workbench.rebuild_controls(cx);
        workbench
    }

    /// Rebuilds one text control per text-like field of the selected command.
    fn rebuild_controls(&mut self, cx: &mut Context<Self>) {
        self.fields.clear();
        self.raw_open = false;
        for (name, placeholder) in self.session.text_fields() {
            let seed = self
                .session
                .state()
                .value(&name)
                .map(|value| value.text.clone())
                .unwrap_or_default();
            let control = cx.new(|cx| {
                let mut field = TextField::new(placeholder, cx);
                if !seed.is_empty() {
                    field.set_text(seed, cx);
                }
                field
            });
            cx.subscribe(&control, |_, _, _: &TextFieldEvent, cx| cx.notify())
                .detach();
            self.fields.insert(name, control);
        }
        cx.notify();
    }

    /// Selects one command and rebuilds its form controls.
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.session.select(index);
        self.run_task = None;
        self.rebuild_controls(cx);
    }

    /// Selects the command with the given exposed tool name.
    ///
    /// Returns whether a command with that name exists.
    pub fn select_command(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        if self.session.select_command(name) {
            self.run_task = None;
            self.rebuild_controls(cx);
            true
        } else {
            false
        }
    }

    /// Returns the exposed name of the selected command.
    pub fn selected_command(&self) -> Option<&str> {
        self.session.selected_command()
    }

    /// Sets one text control's contents.
    ///
    /// Has no effect on a field that is not collected as text, such as a
    /// switch or a choice; drive those through the session's form state.
    pub fn set_field_text(&mut self, name: &str, text: &str, cx: &mut Context<Self>) {
        if let Some(control) = self.fields.get(name) {
            control.update(cx, |field, cx| field.set_text(text.to_string(), cx));
            cx.notify();
        }
    }

    /// Moves keyboard focus to one text control.
    ///
    /// Returns whether that field is collected as text.
    pub fn focus_field(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.fields.get(name) {
            Some(control) => {
                let handle = control.read(cx).focus_handle(cx);
                window.focus(&handle);
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Returns whether a call is in flight.
    pub fn is_running(&self) -> bool {
        self.session.is_running()
    }

    /// Returns the most recent successful result.
    pub fn last_result(&self) -> Option<&Value> {
        self.session.last_result()
    }

    /// Returns the most recent failure message.
    pub fn last_error(&self) -> Option<&str> {
        self.session.last_error()
    }

    /// Sets the agent skill publisher offered by the footer.
    pub fn set_skills(&mut self, skills: Option<SkillPublisher>, cx: &mut Context<Self>) {
        self.session.set_skills(skills);
        cx.notify();
    }

    /// Returns the report of the last successful skill installation.
    pub fn last_skill_report(&self) -> Option<&SkillReport> {
        match self.session.skill_state() {
            SkillState::Installed(report) => Some(report),
            _ => None,
        }
    }

    /// Returns the message of the last failed skill installation.
    pub fn last_skill_error(&self) -> Option<&str> {
        match self.session.skill_state() {
            SkillState::Failed(message) => Some(message),
            _ => None,
        }
    }

    /// Generates and installs this application's agent skills.
    ///
    /// Does nothing when no publisher is configured or one is already running.
    pub fn install_skills(&mut self, cx: &mut Context<Self>) {
        let Some(publisher) = self.session.begin_skill_install() else {
            return;
        };

        // Installation touches the filesystem, so it runs on the call runtime
        // rather than blocking the window.
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.session.runner().spawn(async move {
            let _ = sender.send(match publisher.install().await {
                Ok(result) => Ok(SkillReport::from_result(&result)),
                Err(error) => Err(error.to_string()),
            });
        });

        self.skill_task = Some(cx.spawn(async move |this, cx| {
            let outcome = receiver.await;
            let _ = this.update(cx, |this, cx| {
                this.session.finish_skill_install(match outcome {
                    Ok(result) => result,
                    Err(_) => Err("Installation stopped unexpectedly.".to_string()),
                });
                this.skill_task = None;
                cx.notify();
            });
        }));

        cx.notify();
    }

    /// Copies every text control back into the form state.
    ///
    /// The controls are the source of truth for text, so this must run before
    /// arguments are collected. Deleting it makes typing reach no command.
    fn sync_text_values(&mut self, cx: &mut App) {
        let texts: Vec<(String, String)> = self
            .fields
            .iter()
            .map(|(name, control)| (name.clone(), control.read(cx).text().to_string()))
            .collect();
        for (name, text) in texts {
            self.session.state_mut().value_mut(&name).text = text;
        }
    }

    /// Starts the selected command with the collected values.
    pub fn run_selected(&mut self, cx: &mut Context<Self>) {
        self.sync_text_values(cx);

        let handle = match self.session.start_run() {
            Ok(handle) => handle,
            Err(_) => {
                self.apply_issue_marks(cx);
                cx.notify();
                return;
            }
        };
        self.apply_issue_marks(cx);
        self.raw_open = false;

        let mut updates = handle.updates;
        self.run_task = Some(cx.spawn(async move |this, cx| {
            while let Some(update) = updates.next().await {
                let delivered = this.update(cx, |this, cx| this.receive(update, cx));
                if delivered.is_err() {
                    break;
                }
            }
        }));

        cx.notify();
    }

    /// Applies one ordered update from the running call.
    fn receive(&mut self, update: RunUpdate, cx: &mut Context<Self>) {
        let terminal = matches!(update, RunUpdate::Finished(_));
        self.session.receive(update);
        if terminal {
            self.run_task = None;
            self.apply_issue_marks(cx);
        }
        cx.notify();
    }

    /// Marks each text control according to its current field problem.
    fn apply_issue_marks(&mut self, cx: &mut App) {
        let marks: Vec<(String, bool)> = self
            .fields
            .keys()
            .map(|name| {
                let invalid = self
                    .session
                    .state()
                    .value(name)
                    .and_then(|value| value.issue.as_ref())
                    .is_some();
                (name.clone(), invalid)
            })
            .collect();
        for (name, invalid) in marks {
            if let Some(control) = self.fields.get(&name) {
                control.update(cx, |field, cx| field.set_invalid(invalid, cx));
            }
        }
    }

    /// Cancels the active call, if any.
    fn cancel_run(&mut self, cx: &mut Context<Self>) {
        self.session.cancel();
        cx.notify();
    }

    /// Returns the indices of commands matching the current search text.
    fn visible_commands(&self, cx: &App) -> Vec<usize> {
        self.session.visible(self.search.read(cx).text())
    }
}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Follow the system appearance when the window's appearance changes.
        let appearance = window.appearance();
        if cx.global::<Theme>() != &Theme::for_appearance(appearance) {
            cx.set_global(Theme::for_appearance(appearance));
        }
        let theme = *cx.global::<Theme>();

        div()
            .track_focus(&self.focus_handle)
            .flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .text_size(px(13.0))
            .font_family(".SystemUIFont")
            .child(self.render_sidebar(theme, cx))
            .child(self.render_detail(theme, cx))
    }
}

impl Workbench {
    /// Renders the command list.
    fn render_sidebar(&mut self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible_commands(cx);
        let selected = self.session.selected_index();

        div()
            .w(SIDEBAR_WIDTH)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.app_title.clone()),
                    )
                    .children(self.session.version().map(|version| {
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(format!("Version {version}"))
                    })),
            )
            .child(div().px_3().py_2().child(self.search.clone()))
            .child(
                div()
                    .id("command-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_px()
                    .children(visible.into_iter().map(|index| {
                        let command = &self.session.commands()[index];
                        let is_selected = selected == Some(index);
                        div()
                            .id(("command", index))
                            .px_2()
                            .py_1p5()
                            .rounded(RADIUS)
                            .cursor_pointer()
                            .when(is_selected, |this| this.bg(theme.surface_selected))
                            .when(!is_selected, |this| {
                                this.hover(|style| style.bg(theme.surface_hover))
                            })
                            .child(
                                div()
                                    .font_weight(if is_selected {
                                        gpui::FontWeight::SEMIBOLD
                                    } else {
                                        gpui::FontWeight::NORMAL
                                    })
                                    .child(command.title.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.select(index, cx)))
                    })),
            )
            .children(self.render_skill_footer(theme, cx))
    }

    /// Renders the agent-skill installer, when one is configured.
    ///
    /// Installing is offered rather than performed, because it writes into the
    /// person's agent configuration directories.
    fn render_skill_footer(
        &mut self,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let publisher = self.session.skills()?;
        if publisher.is_empty() {
            return None;
        }
        let installing = matches!(self.session.skill_state(), SkillState::Installing);

        let status: Option<gpui::AnyElement> = match self.session.skill_state() {
            SkillState::Idle => Some(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child("Let your coding agents use these commands.")
                    .into_any_element(),
            ),
            SkillState::Installing => None,
            SkillState::Installed(report) => Some(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.success)
                    .child(report.summary())
                    .into_any_element(),
            ),
            SkillState::Failed(message) => Some(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.danger)
                    .child(message.clone())
                    .into_any_element(),
            ),
        };

        Some(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .p_3()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_muted)
                        .child("Agent skills"),
                )
                .children(status)
                .child(
                    div()
                        .id("install-skills")
                        .px_3()
                        .py_1p5()
                        .rounded(RADIUS)
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(12.0))
                        .when(!installing, |this| {
                            this.cursor_pointer()
                                .hover(|style| style.bg(theme.surface_hover))
                        })
                        .child(match self.session.skill_state() {
                            SkillState::Installing => "Installing…",
                            SkillState::Installed(_) => "Install again",
                            _ => "Install",
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !installing {
                                this.install_skills(cx);
                            }
                        })),
                )
                .into_any_element(),
        )
    }

    /// Renders the selected command's form and result.
    fn render_detail(&mut self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(index) = self.session.selected_index() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_muted)
                .child("This application has no commands to show.")
                .into_any_element();
        };
        let command = &self.session.commands()[index];
        let title = command.title.clone();
        let description = command.description.clone();
        let destructive = command.destructive;
        let model = command.model.clone();
        let running = matches!(self.session.run_state(), RunState::Running { .. });

        div()
            .id("detail")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_6()
                    .max_w(px(760.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .when(!description.is_empty(), |this| {
                                this.child(div().text_color(theme.text_muted).child(description))
                            })
                            .when(destructive, |this| {
                                this.child(
                                    div()
                                        .mt_1()
                                        .px_2()
                                        .py_1()
                                        .rounded(RADIUS)
                                        .bg(theme.danger_surface)
                                        .text_color(theme.danger)
                                        .child("This command makes changes that cannot be undone."),
                                )
                            }),
                    )
                    .child(self.render_form(&model, theme, cx))
                    .child(self.render_actions(running, theme, cx))
                    .child(self.render_result(theme, cx)),
            )
            .into_any_element()
    }

    /// Renders every input control for the selected command.
    fn render_form(
        &mut self,
        model: &FormModel,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if model.is_empty() {
            return div()
                .text_color(theme.text_muted)
                .child("This command needs no information.")
                .into_any_element();
        }

        let rows: Vec<_> = model
            .fields
            .iter()
            .map(|field| {
                let value = self
                    .session
                    .state()
                    .value(&field.name)
                    .cloned()
                    .unwrap_or_default();
                let name = field.name.clone();

                let control: gpui::AnyElement = match &field.kind {
                    FieldKind::Toggle => {
                        let on = value.toggle;
                        div()
                            .id(SharedString::from(format!("toggle:{name}")))
                            .w(px(44.0))
                            .h(px(26.0))
                            .p(px(3.0))
                            .rounded(px(13.0))
                            .cursor_pointer()
                            .bg(if on { theme.accent } else { theme.border })
                            .flex()
                            .when(on, |this| this.justify_end())
                            .child(div().size(px(20.0)).rounded(px(10.0)).bg(theme.surface))
                            .on_click(cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| {
                                    let value = this.session.state_mut().value_mut(&name);
                                    value.toggle = !value.toggle;
                                    cx.notify();
                                }
                            }))
                            .into_any_element()
                    }
                    FieldKind::Select { options } => div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(options.iter().map(|option| {
                            let chosen = value.choice == *option;
                            div()
                                .id(SharedString::from(format!("choice:{name}:{option}")))
                                .px_3()
                                .py_1()
                                .rounded(RADIUS)
                                .cursor_pointer()
                                .border_1()
                                .border_color(if chosen { theme.accent } else { theme.border })
                                .bg(if chosen { theme.accent } else { theme.field })
                                .text_color(if chosen {
                                    theme.text_on_accent
                                } else {
                                    theme.text
                                })
                                .child(option.clone())
                                .on_click(cx.listener({
                                    let name = name.clone();
                                    let option = option.clone();
                                    move |this, _, _, cx| {
                                        this.session.state_mut().value_mut(&name).choice =
                                            option.clone();
                                        cx.notify();
                                    }
                                }))
                        }))
                        .into_any_element(),
                    _ => match self.fields.get(&name) {
                        Some(control) => control.clone().into_any_element(),
                        None => div().h(FIELD_HEIGHT).into_any_element(),
                    },
                };

                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(field.label.clone()),
                            )
                            .when(field.required, |this| {
                                this.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(theme.text_muted)
                                        .child("Required"),
                                )
                            }),
                    )
                    .children(field.help.clone().map(|help| {
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(help)
                    }))
                    .child(control)
                    .children(value.issue.clone().map(|issue| {
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.danger)
                            .child(issue)
                    }))
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(rows)
            .into_any_element()
    }

    /// Renders the run and cancel controls.
    fn render_actions(
        &mut self,
        running: bool,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("run")
                    .px_4()
                    .py_2()
                    .rounded(RADIUS)
                    .cursor_pointer()
                    .bg(if running { theme.border } else { theme.accent })
                    .text_color(theme.text_on_accent)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .when(!running, |this| {
                        this.hover(|style| style.bg(theme.accent_hover))
                    })
                    .child(if running { "Running…" } else { "Run" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !running {
                            this.run_selected(cx);
                        }
                    })),
            )
            .when(running, |this| {
                this.child(
                    div()
                        .id("cancel")
                        .px_3()
                        .py_2()
                        .rounded(RADIUS)
                        .cursor_pointer()
                        .border_1()
                        .border_color(theme.border)
                        .child("Stop")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_run(cx))),
                )
            })
    }

    /// Renders the current run state.
    fn render_result(&mut self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.session.run_state() {
            RunState::Idle => div().into_any_element(),
            RunState::Cancelled => card(theme)
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .child("Stopped before it finished."),
                )
                .into_any_element(),
            RunState::Running {
                log,
                fraction,
                chunks,
                ..
            } => {
                let lines: Vec<String> = log.iter().rev().take(8).rev().cloned().collect();
                card(theme)
                    .child(div().text_color(theme.text_muted).child(match fraction {
                        Some(value) => {
                            format!("Working… {}%", (value * 100.0).round() as i64)
                        }
                        None => "Working…".to_string(),
                    }))
                    .when(!chunks.is_empty(), |this| {
                        this.child(
                            div()
                                .text_color(theme.text_muted)
                                .child(format!("{} results so far", chunks.len())),
                        )
                    })
                    .children(lines.into_iter().map(|line| {
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(line)
                    }))
                    .into_any_element()
            }
            RunState::Failed {
                message,
                code,
                details,
            } => card(theme)
                .bg(theme.danger_surface)
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.danger)
                        .child(message.clone()),
                )
                .children(
                    details
                        .iter()
                        .map(|detail| div().text_color(theme.danger).child(detail.clone())),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(code.clone()),
                )
                .into_any_element(),
            RunState::Succeeded { data, cta } => {
                let raw_open = self.raw_open;
                let rows = rows_for(data);
                let json = raw_json(data);
                card(theme)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.success)
                            .child("Done"),
                    )
                    .when(rows.is_empty(), |this| {
                        this.child(
                            div()
                                .text_color(theme.text_muted)
                                .child("Finished with nothing to show."),
                        )
                    })
                    .children(rows.into_iter().map(|row| render_row(row, theme)))
                    .children(cta.as_ref().map(|cta| render_cta(cta, theme)))
                    .child(
                        div()
                            .id("raw-toggle")
                            .mt_1()
                            .text_size(px(11.0))
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .child(if raw_open {
                                "Hide raw data"
                            } else {
                                "Show raw data"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.raw_open = !this.raw_open;
                                cx.notify();
                            })),
                    )
                    .when(raw_open, |this| {
                        this.child(
                            div()
                                .id("raw-json")
                                .max_h(px(280.0))
                                .overflow_y_scroll()
                                .p_2()
                                .rounded(RADIUS)
                                .bg(theme.background)
                                .font_family("monospace")
                                .text_size(px(11.0))
                                .child(json),
                        )
                    })
                    .into_any_element()
            }
        }
    }
}

/// Renders one result row at its indentation depth.
fn render_row(row: DisplayRow, theme: Theme) -> impl IntoElement {
    let indent = px(row.depth as f32 * 14.0);
    div()
        .flex()
        .gap_2()
        .pl(indent)
        .child(
            div()
                .min_w(px(120.0))
                .text_color(theme.text_muted)
                .child(row.label),
        )
        .children(row.value.map(|value| div().flex_1().child(value)))
}

/// Renders the command's follow-up suggestions.
fn render_cta(cta: &CtaBlock, theme: Theme) -> impl IntoElement {
    let label = cta
        .description
        .clone()
        .unwrap_or_else(|| "Next steps".to_string());
    div()
        .mt_2()
        .pt_2()
        .border_t_1()
        .border_color(theme.border)
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_muted)
                .child(label),
        )
        .children(cta.commands.iter().map(|entry| {
            let text = match entry {
                CtaEntry::Simple(command) => command.clone(),
                CtaEntry::Detailed {
                    command,
                    description,
                } => match description {
                    Some(description) => format!("{command} — {description}"),
                    None => command.clone(),
                },
            };
            div().text_size(px(11.0)).child(text)
        }))
}

/// Builds the shared result card container.
fn card(theme: Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded(RADIUS)
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border)
}
