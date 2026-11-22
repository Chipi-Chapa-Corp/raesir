use crate::{Launcher, views::text_button};
use gpui::{prelude::*, *};
use raesir_backend::mods::nexus_account as provider;

#[derive(Default)]
enum Status {
    #[default]
    Unchecked,
    Checking,
    SignedOut,
    OpeningBrowser,
    WaitingForApproval,
    SignedIn(String),
}

#[derive(Default)]
pub(crate) struct NexusAccount {
    status: Status,
    task: Option<Task<()>>,
    error: Option<String>,
}

impl NexusAccount {
    pub(crate) fn banner(&self) -> Option<(&str, &str)> {
        match &self.status {
            Status::SignedOut => Some((
                self.error
                    .as_deref()
                    .unwrap_or("Connect your Nexus Mods account."),
                "Sign in to Nexus Mods",
            )),
            Status::OpeningBrowser => Some(("Connecting to Nexus Mods…", "Signing in…")),
            Status::WaitingForApproval => Some(("Approve Raesir in your browser.", "Signing in…")),
            _ => None,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.task.is_some()
    }

    pub(crate) fn signed_in(&self) -> bool {
        matches!(self.status, Status::SignedIn(_))
    }
}

impl Launcher {
    pub(crate) fn check_nexus_account(&mut self, cx: &mut Context<Self>) {
        if !(provider::configured() || provider::testing())
            || !matches!(self.nexus_account.status, Status::Unchecked)
        {
            return;
        }
        if !self.backend.nexus_actions_allowed() {
            self.nexus_account.status = Status::SignedOut;
            return;
        }
        self.nexus_account.status = Status::Checking;
        let executor = cx.background_executor().clone();
        self.nexus_account.task = Some(cx.spawn(async move |this, cx| {
            let result = executor.spawn(async { provider::current() }).await;
            this.update(cx, |this, cx| this.finish_nexus_account(result, cx))
                .ok();
        }));
    }

    pub(crate) fn sign_in_nexus(&mut self, cx: &mut Context<Self>) {
        if !provider::configured()
            || self.nexus_account.busy()
            || matches!(self.nexus_account.status, Status::SignedIn(_))
        {
            return;
        }
        if !self.backend.nexus_actions_allowed() {
            self.finish_nexus_account(
                Err("Nexus Mods sign-in is disabled in this session".into()),
                cx,
            );
            return;
        }
        self.nexus_account.status = Status::OpeningBrowser;
        self.nexus_account.error = None;
        let executor = cx.background_executor().clone();
        let backend = self.backend.clone();
        self.nexus_account.task = Some(cx.spawn(async move |this, cx| {
            let sign_in = match executor.spawn(async { provider::SignIn::start() }).await {
                Ok(sign_in) => sign_in,
                Err(error) => {
                    this.update(cx, |this, cx| this.finish_nexus_account(Err(error), cx))
                        .ok();
                    return;
                }
            };
            if !this
                .update(cx, |this, cx| {
                    if !this.backend.nexus_actions_allowed() {
                        this.finish_nexus_account(
                            Err("Nexus Mods sign-in was cancelled".into()),
                            cx,
                        );
                        return false;
                    }
                    cx.open_url(sign_in.url());
                    this.nexus_account.status = Status::WaitingForApproval;
                    this.notify_nexus_account(cx);
                    true
                })
                .unwrap_or(false)
            {
                return;
            }
            let result = executor
                .spawn(async move { sign_in.finish(|| !backend.nexus_actions_allowed()) })
                .await;
            this.update(cx, |this, cx| {
                this.finish_nexus_account(result.map(Some), cx)
            })
            .ok();
        }));
        self.notify_nexus_account(cx);
    }

    fn sign_out_nexus(&mut self, cx: &mut Context<Self>) {
        if self.nexus_account.busy() || !self.backend.nexus_actions_allowed() {
            return;
        }
        let executor = cx.background_executor().clone();
        self.nexus_account.task = Some(cx.spawn(async move |this, cx| {
            let result = executor.spawn(async { provider::sign_out() }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.finish_nexus_account(Ok(None), cx),
                Err(error) => {
                    this.nexus_account.task = None;
                    this.nexus_account.error = Some(error);
                    this.notify_nexus_account(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn finish_nexus_account(
        &mut self,
        result: Result<Option<String>, String>,
        cx: &mut Context<Self>,
    ) {
        self.nexus_account.task = None;
        self.nexus_account.error = None;
        self.nexus_account.status = match result {
            Ok(Some(name)) => Status::SignedIn(name),
            Ok(None) => Status::SignedOut,
            Err(error) => {
                self.nexus_account.error = Some(error);
                Status::SignedOut
            }
        };
        self.notify_nexus_account(cx);
    }

    fn notify_nexus_account(&self, cx: &mut Context<Self>) {
        if let Some(mods) = &self.mods {
            mods.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    pub(crate) fn nexus_account_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let account = &self.nexus_account;
        if matches!(account.status, Status::Unchecked) {
            let launcher = cx.weak_entity();
            cx.defer(move |cx| {
                launcher
                    .update(cx, |this, cx| this.check_nexus_account(cx))
                    .ok();
            });
        }
        let signed_in = matches!(account.status, Status::SignedIn(_));
        let label = if signed_in { "Sign out" } else { "Sign in" };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(match &account.status {
                        Status::SignedIn(name) => format!("Nexus Mods · {name}"),
                        _ => "Nexus Mods account".into(),
                    })
                    .child(text_button("nexus-account-setting", label).when(
                        !account.busy(),
                        |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                if signed_in {
                                    this.sign_out_nexus(cx);
                                } else {
                                    this.sign_in_nexus(cx);
                                }
                            }))
                        },
                    )),
            )
            .when_some(account.error.clone(), |d, error| {
                d.child(
                    div()
                        .whitespace_normal()
                        .text_color(rgb(0xffc4ab))
                        .child(error),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{NexusAccount, Status};

    #[test]
    fn nexus_sign_in_prompt_follows_account_state() {
        let mut account = NexusAccount::default();
        assert!(account.banner().is_none());
        account.status = Status::SignedOut;
        assert!(account.banner().is_some());
        account.status = Status::WaitingForApproval;
        assert_eq!(
            account.banner().unwrap().0,
            "Approve Raesir in your browser."
        );
        account.status = Status::SignedIn("Fixture".into());
        assert!(account.banner().is_none());
    }
}
