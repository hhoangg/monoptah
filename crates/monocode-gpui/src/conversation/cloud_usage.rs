//! The muted "This month: ..." line under the Machine row of the Codespace panel: how much
//! Codespaces compute and storage the user has used this month and what GitHub will bill for
//! it, so the cost of a new codespace is visible before it is created.
//!
//! The numbers come from `monocode_core::codespaces::usage` through a `UsageLoader` the shell
//! installs (it holds the 10-minute cache). It is asked when the Codespace panel opens and when
//! the user presses the refresh button, never while rendering or typing, and nothing waits for
//! it: Send does not look at this state. A failure shows a short hint instead of numbers.
use super::cloud_panel::Tip;
use super::cloud_target::*;
use super::*;
use monocode_core::codespaces::usage::{BILLING_SUMMARY_URL, CodespacesUsage, UsageProblem};

/// Reads this month's usage on a worker. The flag asks for a fresh answer (the refresh button);
/// otherwise a cached one is fine. Blocking.
pub(crate) type UsageLoader =
    Arc<dyn Fn(bool) -> Result<CodespacesUsage, UsageProblem> + Send + Sync>;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum UsageView {
    /// Nothing asked yet (or no loader): the row is not shown.
    #[default]
    Idle,
    Loading,
    Ready(CodespacesUsage),
    Unavailable(UsageProblem),
}

#[derive(Default)]
pub(super) struct UsageState {
    pub loader: Option<UsageLoader>,
    pub view: UsageView,
    /// A read is running: the refresh button waits, and the last numbers stay meanwhile.
    pub refreshing: bool,
    /// Which read the answer belongs to.
    pub request: u64,
}

/// The pieces of the line, as text.
#[derive(Debug, PartialEq)]
pub(super) struct UsageText {
    /// "This month: 5.8 of 180 core-hours, $0.52 of free usage used, $0.00 billed"
    pub facts: String,
    /// "$2.20 billed to your account", in the warning colour. Replaces the "$0.00 billed" tail.
    pub billed: Option<String>,
    /// "Estimate: ~87 h left on a 2-core machine (assumes 180 core-hours included with Pro)"
    pub estimate: Option<String>,
}

fn money(amount: f64) -> String {
    format!("${:.2}", amount.max(0.))
}

fn hours(value: f64) -> String {
    if value >= 10. {
        format!("{value:.0} h")
    } else {
        format!("{value:.1} h")
    }
}

/// A count of core-hours or GB-months without a trailing ".0" for round allowances.
fn amount(value: f64) -> String {
    if value >= 10. || value.fract() == 0. {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn plan_label(plan: &str) -> String {
    let mut chars = plan.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The line for `usage`. `cores` is the size of the selected machine when known; it only feeds
/// the estimate. Compute is shown in core-hours, the unit of GitHub's allowance (a 4-core
/// machine used for an hour is 4 of them).
pub(super) fn usage_text(usage: &CodespacesUsage, cores: Option<u32>) -> UsageText {
    let billed = usage.is_billed();
    let used = amount(usage.compute_core_hours);
    let compute = match usage.included_core_hours() {
        Some(included) => format!("{used} of {} core-hours", amount(included)),
        None => format!("{used} core-hours"),
    };
    let mut facts = format!(
        "This month: {compute}, {} of free usage used",
        money(usage.discount)
    );
    if !billed {
        facts.push_str(", $0.00 billed");
    }
    let estimate = cores
        .and_then(|cores| usage.estimated_hours_left(cores).map(|left| (cores, left)))
        .zip(usage.included_core_hours().zip(usage.plan.as_deref()))
        .map(|((cores, left), (included, plan))| {
            format!(
                "Estimate: ~{} left on a {cores}-core machine (assumes {} core-hours included with {})",
                hours(left),
                amount(included),
                plan_label(plan)
            )
        });
    UsageText {
        facts,
        billed: billed.then(|| format!("{} billed to your account", money(usage.net))),
        estimate,
    }
}

/// The hover text: what the numbers are and where the authoritative ones are.
pub(super) fn usage_tooltip(usage: Option<&CodespacesUsage>) -> String {
    let mut text = String::from(
        "Your Codespaces usage this month from GitHub's billing summary. \"Free usage\" is the \
         part covered by your included allowance or credits; \"billed\" is what GitHub will \
         charge your account.",
    );
    if let Some(usage) = usage {
        text.push_str(&format!(
            " Compute is counted in core-hours like GitHub's allowance: {} machine-hours, \
             {} core-hours (a 4-core machine for 1 hour is 4). Gross {}, discount {}, net {}; \
             storage {:.1} GB-months.",
            amount(usage.compute_machine_hours),
            amount(usage.compute_core_hours),
            money(usage.gross),
            money(usage.discount),
            money(usage.net),
            usage.storage_gb_months
        ));
        if let (Some(included), Some(storage)) = (
            usage.included_core_hours(),
            usage.included_storage_gb_months(),
        ) {
            text.push_str(&format!(
                " The estimate assumes {} core-hours and {} GB-months of Codespaces included \
                 with your plan. Actions minutes are a separate allowance. These figures are \
                 from GitHub's documentation, not the API, and are unverified.",
                amount(included),
                amount(storage)
            ));
        }
    }
    text.push_str(" GitHub's billing page is authoritative: use the link on the right.");
    text
}

/// The cores of the machine a new codespace would get: the pick, else the smallest listed.
fn selected_cores(draft: &CloudDraft) -> Option<u32> {
    let Machines::Loaded(list) = &draft.machines else {
        return None;
    };
    let machine = match draft.machine.as_deref() {
        Some(name) => list.iter().find(|m| m.name == name),
        None => list.iter().filter(|m| m.cpus > 0).min_by_key(|m| m.cpus),
    };
    machine.map(|m| m.cpus).filter(|cores| *cores > 0)
}

impl Conversation {
    /// The shell tells how to read the Codespaces usage (and caches it).
    pub(crate) fn set_usage_loader(&mut self, loader: UsageLoader) {
        self.cloud_usage.loader = Some(loader);
    }

    /// Reads the usage on a worker. Called when the Codespace panel opens and by the refresh
    /// button (`force`); the answer only applies to the latest read.
    pub(super) fn load_codespace_usage(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(loader) = self.cloud_usage.loader.clone() else {
            return;
        };
        if self.cloud_usage.refreshing && !force {
            return;
        }
        self.cloud_usage.request += 1;
        let request = self.cloud_usage.request;
        self.cloud_usage.refreshing = true;
        // The last numbers stay on screen while a refresh runs.
        if !matches!(self.cloud_usage.view, UsageView::Ready(_)) {
            self.cloud_usage.view = UsageView::Loading;
        }
        let work = cx.background_executor().spawn(async move { loader(force) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.cloud_usage.request == request {
                    this.cloud_usage.refreshing = false;
                    this.cloud_usage.view = match result {
                        Ok(usage) => UsageView::Ready(usage),
                        Err(problem) => UsageView::Unavailable(problem),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The usage line of a Codespace panel; nothing until a read was asked for.
    pub(super) fn cloud_usage_row(
        &self,
        draft: &CloudDraft,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = &self.cloud_usage;
        let muted = content(0.45);
        let usage = match &state.view {
            UsageView::Ready(usage) => Some(usage),
            _ => None,
        };
        let mut line = div()
            .id("cloud-usage-text")
            .debug_selector(|| "cloud-usage-text".into())
            .flex_1()
            .min_w_0()
            .flex()
            .flex_wrap()
            .gap_x(px(4.))
            .text_size(px(11.))
            .line_height(px(15.))
            .text_color(muted);
        match &state.view {
            UsageView::Idle => return None,
            UsageView::Loading => line = line.child("This month: loading usage..."),
            UsageView::Unavailable(problem) => line = line.child(problem.hint()),
            UsageView::Ready(usage) => {
                let text = usage_text(usage, selected_cores(draft));
                line = line.child(text.facts);
                if let Some(billed) = text.billed {
                    line = line.child(
                        div()
                            .id("cloud-usage-billed")
                            .debug_selector(|| "cloud-usage-billed".into())
                            .text_color(warning_color())
                            .child(billed),
                    );
                }
                if let Some(estimate) = text.estimate {
                    line = line.child(
                        div()
                            .id("cloud-usage-estimate")
                            .debug_selector(|| "cloud-usage-estimate".into())
                            .child(estimate),
                    );
                }
            }
        }
        let tip: SharedString = usage_tooltip(usage).into();
        let line = line.tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into());
        let busy = state.refreshing || self.cloud_creating();
        Some(
            div()
                .id("cloud-usage")
                .debug_selector(|| "cloud-usage".into())
                .role(Role::Group)
                .aria_label("Codespaces usage this month")
                .pl(px(34.))
                .pb(px(6.))
                .flex()
                .items_start()
                .gap(px(4.))
                .child(line)
                .child(
                    icon_button_enabled(
                        "cloud-usage-refresh",
                        "Refresh usage",
                        "refresh-cw",
                        !busy,
                    )
                    .size(px(20.))
                    .when(!busy, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| {
                            this.load_codespace_usage(true, cx);
                        }))
                    }),
                )
                .child(
                    icon_button(
                        "cloud-usage-open",
                        "Open GitHub billing summary",
                        "external-link",
                    )
                    .size(px(20.))
                    .on_click(|_, _, cx| cx.open_url(BILLING_SUMMARY_URL)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
