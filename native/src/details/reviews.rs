use super::*;
use exposed_games_backend::details::{
    Review,
    reviews::{Page, Query, SteamReviews},
};
use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, text_input};

const LANGUAGES: &[(&str, &str)] = &[
    ("English", "english"),
    ("All languages", "all"),
    ("Arabic", "arabic"),
    ("Bulgarian", "bulgarian"),
    ("Chinese (Simplified)", "schinese"),
    ("Chinese (Traditional)", "tchinese"),
    ("Czech", "czech"),
    ("Danish", "danish"),
    ("Dutch", "dutch"),
    ("Finnish", "finnish"),
    ("French", "french"),
    ("German", "german"),
    ("Greek", "greek"),
    ("Hungarian", "hungarian"),
    ("Indonesian", "indonesian"),
    ("Italian", "italian"),
    ("Japanese", "japanese"),
    ("Korean", "koreana"),
    ("Norwegian", "norwegian"),
    ("Polish", "polish"),
    ("Portuguese", "portuguese"),
    ("Portuguese (Brazil)", "brazilian"),
    ("Romanian", "romanian"),
    ("Russian", "russian"),
    ("Spanish (Spain)", "spanish"),
    ("Spanish (Latin America)", "latam"),
    ("Swedish", "swedish"),
    ("Thai", "thai"),
    ("Turkish", "turkish"),
    ("Ukrainian", "ukrainian"),
    ("Vietnamese", "vietnamese"),
];
fn options(kind: usize) -> Vec<&'static str> {
    match kind {
        0 => vec!["All reviews", "Positive", "Negative"],
        1 => vec!["All purchases", "Steam purchases", "Other purchases"],
        2 => LANGUAGES.iter().map(|(name, _)| *name).collect(),
        3 => vec![
            "All dates",
            "Past day",
            "Past week",
            "Past 30 days",
            "Past year",
            "Custom range",
        ],
        4 => vec![
            "Any playtime",
            "Up to 1 hour",
            "1–10 hours",
            "10–100 hours",
            "100+ hours",
            "Custom range",
        ],
        _ => vec![
            "Summary",
            "Most helpful",
            "Most recent",
            "Recently updated",
            "Funniest",
        ],
    }
}

pub(super) struct State {
    selected: [usize; 6],
    inputs: [Entity<EditableTextState>; 4],
    query: Query,
    pages: Option<SteamReviews>,
    loading: [bool; 2],
    errors: [Option<String>; 2],
    tasks: [Option<Task<()>>; 2],
    generation: u64,
    expanded: HashSet<String>,
    recent_visible: usize,
    menu_scroll: ScrollHandle,
}
impl State {
    pub fn new(cx: &mut Context<DetailsPage>) -> Self {
        Self {
            selected: [0; 6],
            inputs: std::array::from_fn(|_| {
                cx.new(|cx| EditableTextState::new(StringStorage::default(), cx))
            }),
            query: Query::default(),
            pages: None,
            loading: [false; 2],
            errors: [None, None],
            tasks: [None, None],
            generation: 0,
            expanded: HashSet::new(),
            recent_visible: 4,
            menu_scroll: ScrollHandle::new(),
        }
    }
}

impl DetailsPage {
    fn steam_reviews(&self) -> bool {
        self.data
            .as_ref()
            .is_some_and(|d| d.steam_reviews.is_some())
    }
    fn review_options(&self, kind: usize) -> Vec<&'static str> {
        let mut labels = options(kind);
        if kind == 3 && !self.steam_reviews() {
            labels.truncate(5);
        }
        labels
    }
    fn select_filter(
        &mut self,
        kind: usize,
        value: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.review_state.selected[kind] = value;
        self.reviews_visible = 4;
        self.review_state.recent_visible = 4;
        self.close_filter(window, cx);
        if self.steam_reviews() && !matches!((kind, value), (3 | 4, 5)) {
            self.apply_review_filters(cx);
        }
    }
    fn apply_review_filters(&mut self, cx: &mut Context<Self>) {
        let selected = self.review_state.selected;
        let mut query = Query {
            review_type: selected[0] as u8,
            purchase_type: [1, 0, 2][selected[1]],
            language: LANGUAGES[selected[2]].1.into(),
            display: selected[5] as u8,
            filter_offtopic: self.review_state.query.filter_offtopic,
            use_review_quality: self.review_state.query.use_review_quality,
            ..Default::default()
        };
        let custom = |index: usize| {
            self.review_state.inputs[index]
                .read(cx)
                .as_str()
                .trim()
                .to_owned()
        };
        let dates = if selected[3] == 5 {
            exposed_games_backend::details::reviews::date_range(&custom(0), &custom(1))
        } else {
            let days = [0, 1, 7, 30, 365][selected[3]];
            Ok(if days == 0 {
                (0, 0)
            } else {
                let end = exposed_games_backend::details::now().min(u32::MAX as u64) as u32;
                (end.saturating_sub(days * 86400), end)
            })
        };
        let playtime = if selected[4] == 5 {
            (|| {
                let min = custom(2)
                    .parse::<u32>()
                    .map_err(|_| "Enter a minimum number of whole hours.")?;
                let max = custom(3)
                    .parse::<u32>()
                    .map_err(|_| "Enter a maximum number of whole hours.")?;
                if min > max || max == 0 {
                    return Err(
                        "The maximum playtime must be at least the minimum and greater than zero.",
                    );
                }
                Ok((min, max))
            })()
            .map_err(str::to_owned)
        } else {
            Ok([(0, 0), (0, 1), (1, 10), (10, 100), (100, 0)][selected[4]])
        };
        let ((date_start, date_end), (min, max)) =
            match dates.and_then(|d| playtime.map(|p| (d, p))) {
                Ok(ranges) => ranges,
                Err(error) => {
                    self.review_state.errors[0] = Some(error);
                    cx.notify();
                    return;
                }
            };
        query.date_start = date_start;
        query.date_end = date_end;
        query.playtime_min = min;
        query.playtime_max = max;
        self.review_state.query = query;
        self.review_state.generation += 1;
        self.review_state.pages = self
            .data
            .as_ref()
            .and_then(|d| d.steam_reviews.clone())
            .map(|p| SteamReviews {
                appid: p.appid,
                ..Default::default()
            });
        self.review_state.expanded.clear();
        self.reviews_visible = 4;
        self.review_state.recent_visible = 4;
        self.review_state.tasks = [None, None];
        self.review_state.loading = [false; 2];
        self.review_state.errors = [None, None];
        self.request_reviews(false, false, cx);
        if self.review_state.query.display == 0 {
            self.request_reviews(true, false, cx);
        }
        cx.notify();
    }
    fn request_reviews(&mut self, recent: bool, append: bool, cx: &mut Context<Self>) {
        let index = usize::from(recent);
        if self.review_state.loading[index] {
            return;
        }
        let Some(pages) = self
            .review_state
            .pages
            .as_ref()
            .or_else(|| self.data.as_ref()?.steam_reviews.as_ref())
        else {
            return;
        };
        let appid = pages.appid.clone();
        let cursor = if append {
            if recent { &pages.recent } else { &pages.main }
                .cursor
                .clone()
        } else {
            Some("*".into())
        };
        let Some(cursor) = cursor else {
            return;
        };
        if self.review_state.pages.is_none() {
            self.review_state.pages = Some(pages.clone());
        }
        let backend = self.backend.clone();
        let query = self.review_state.query.clone();
        let generation = self.review_state.generation;
        self.review_state.loading[index] = true;
        self.review_state.errors[index] = None;
        self.review_state.tasks[index] = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend.review_page(&appid, &query, recent, &cursor) })
                .await;
            this.update(cx, |this, cx| {
                if this.review_state.generation != generation {
                    return;
                }
                this.review_state.loading[index] = false;
                match result {
                    Ok(page) => {
                        if let Some(pages) = &mut this.review_state.pages {
                            let target = if recent {
                                &mut pages.recent
                            } else {
                                &mut pages.main
                            };
                            if append {
                                target.append(page);
                            } else {
                                *target = page;
                            }
                        }
                        if append {
                            if recent {
                                this.review_state.recent_visible += 4;
                            } else {
                                this.reviews_visible += 4;
                            }
                        }
                    }
                    Err(error) => this.review_state.errors[index] = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
    fn review_controls(&self, cx: &mut Context<Self>) -> Div {
        let steam = self.steam_reviews();
        div()
            .flex()
            .flex_wrap()
            .gap(rems(0.65))
            .children(
                (0..6)
                    .filter(|kind| steam || matches!(kind, 0 | 3))
                    .map(|kind| {
                        let selected = self.review_state.selected[kind];
                        let bounds = self.filter_bounds[kind].clone();
                        text_button(("review-filter", kind), options(kind)[selected])
                            .w(rems(12.5))
                            .flex_none()
                            .justify_between()
                            .child(icon("expand"))
                            .on_prepaint(move |paint, _, _| bounds.set(paint.bounds))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.filter_menu.is_some_and(|(open, _)| open == kind) {
                                    this.close_filter(window, cx);
                                } else {
                                    this.filter_menu = Some((kind, selected));
                                    this.review_state.menu_scroll.scroll_to_item(selected);
                                    window.focus(&this.filter_focus, cx);
                                    cx.notify();
                                }
                            }))
                    }),
            )
            .when(steam, |d| {
                d.children(
                    [
                        (
                            "Exclude off-topic activity",
                            self.review_state.query.filter_offtopic,
                        ),
                        (
                            "Prioritize helpful reviews",
                            self.review_state.query.use_review_quality,
                        ),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(i, (label, checked))| {
                        text_button(("review-toggle", i), label)
                            .flex_none()
                            .when(checked, |d| d.child(icon("check")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if i == 0 {
                                    this.review_state.query.filter_offtopic =
                                        !this.review_state.query.filter_offtopic;
                                } else {
                                    this.review_state.query.use_review_quality =
                                        !this.review_state.query.use_review_quality;
                                }
                                this.apply_review_filters(cx);
                            }))
                    }),
                )
            })
    }
    pub(super) fn review_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((kind, highlighted)) = self.filter_menu else {
            return div().into_any_element();
        };
        let selected = self.review_state.selected[kind];
        let bounds = self.filter_bounds[kind].get();
        let labels = self.review_options(kind);
        let height = labels.len().min(8) as f32 * 2.8 + 1.;
        let above = bounds.bottom() + window.rem_size() * height > window.viewport_size().height;
        let origin = point(
            bounds.left(),
            if above {
                bounds.top() - window.rem_size() * 0.4
            } else {
                bounds.bottom() + window.rem_size() * 0.4
            },
        );
        let menu = pill()
            .id("review-filter-menu")
            .occlude()
            .track_focus(&self.filter_focus)
            .w(rems(16.))
            .p(rems(0.5))
            .rounded(rems(1.))
            .shadow_lg()
            .bg(rgba(0x101010f2))
            .flex_col()
            .items_stretch()
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if !this
                    .filter_bounds
                    .iter()
                    .any(|b| b.get().contains(&event.position))
                {
                    this.close_filter(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &crate::ClearSearch, window, cx| {
                this.close_filter(window, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let Some((kind, active)) = this.filter_menu else {
                    return;
                };
                let count = this.review_options(kind).len();
                match event.keystroke.key.as_str() {
                    "up" => this.filter_menu = Some((kind, (active + count - 1) % count)),
                    "down" => this.filter_menu = Some((kind, (active + 1) % count)),
                    "home" => this.filter_menu = Some((kind, 0)),
                    "end" => this.filter_menu = Some((kind, count - 1)),
                    "enter" | "space" => this.select_filter(kind, active, window, cx),
                    "escape" | "tab" => this.close_filter(window, cx),
                    _ => return,
                }
                if let Some((_, active)) = this.filter_menu {
                    this.review_state.menu_scroll.scroll_to_item(active);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                div()
                    .id("review-filter-options")
                    .track_scroll(&self.review_state.menu_scroll)
                    .max_h(rems(21.))
                    .overflow_y_scroll()
                    .children(labels.into_iter().enumerate().map(|(index, label)| {
                        text_button(("review-option", index), label)
                            .w_full()
                            .rounded(rems(0.6))
                            .justify_between()
                            .bg(if index == highlighted {
                                rgba(0x303030b3)
                            } else {
                                rgba(0x00000000)
                            })
                            .when(index == selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(
                                div()
                                    .size(rems(1.25))
                                    .when(index == selected, |d| d.child(icon("check"))),
                            )
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.filter_menu != Some((kind, index)) {
                                    this.filter_menu = Some((kind, index));
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_filter(kind, index, window, cx)
                            }))
                    })),
            );
        deferred(
            anchored()
                .anchor(if above {
                    Anchor::BottomLeft
                } else {
                    Anchor::TopLeft
                })
                .position(origin)
                .snap_to_window_with_margin(window.rem_size() * 0.5)
                .child(menu),
        )
        .priority(1)
        .into_any_element()
    }
    fn custom_review_range(&self, date: bool, cx: &mut Context<Self>) -> Div {
        let base = if date { 0 } else { 2 };
        let labels = if date {
            ["Start date (YYYY-MM-DD)", "End date (YYYY-MM-DD)"]
        } else {
            ["Minimum hours", "Maximum hours"]
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rems(0.65))
            .child(muted(if date {
                "Date range"
            } else {
                "Playtime at review"
            }))
            .children((0..2).map(|i| {
                pill().w(rems(17.)).px(rems(0.8)).child(
                    text_input(("review-range-input", base + i))
                        .state(self.review_state.inputs[base + i].downgrade())
                        .placeholder(labels[i])
                        .whitespace_nowrap()
                        .overflow_x_scroll()
                        .w_full()
                        .flex_1()
                        .min_w_0()
                        .py(rems(0.6))
                        .caret_color(rgb_to_hsla(rgb(0xf0f8f3)))
                        .selection_color(rgb_to_hsla(rgba(0xffffff40)))
                        .caret_blink_interval_500ms(),
                )
            }))
            .child(
                text_button(
                    if date {
                        "apply-review-dates"
                    } else {
                        "apply-review-hours"
                    },
                    "Apply",
                )
                .on_click(cx.listener(|this, _, _, cx| this.apply_review_filters(cx))),
            )
    }
    pub(super) fn reviews_content(&self, data: &Details, cx: &mut Context<Self>) -> Div {
        let mut content = div()
            .flex()
            .flex_col()
            .gap(rems(0.9))
            .w_full()
            .min_w_0()
            .child(heading("Player reviews"))
            .child(self.review_controls(cx));
        if let Some(pages) = self
            .review_state
            .pages
            .as_ref()
            .or(data.steam_reviews.as_ref())
        {
            let summary = self.review_state.query.display == 0;
            if self.review_state.selected[3] == 5 {
                content = content.child(self.custom_review_range(true, cx));
            }
            if self.review_state.selected[4] == 5 {
                content = content.child(self.custom_review_range(false, cx));
            }
            let columns = div()
                .w_full()
                .min_w_0()
                .flex()
                .items_start()
                .gap(rems(1.2))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(self.review_column(&pages.main, false, cx)),
                )
                .when(summary, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .w(relative(0.34))
                            .flex_none()
                            .min_w_0()
                            .child(self.review_column(&pages.recent, true, cx)),
                    )
                });
            content = content.child(columns);
        } else {
            let since = exposed_games_backend::details::review_cutoff(
                [0, 1, 7, 30, 365, 0][self.review_state.selected[3]],
            );
            let recommendation = match self.review_state.selected[0] {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            };
            let matching = data
                .reviews
                .iter()
                .filter(|r| r.matches_filters(&since, recommendation))
                .collect::<Vec<_>>();
            if matching.is_empty() {
                content = content.child(panel().child(muted("No reviews match these filters.")));
            }
            content = content
                .children(
                    matching
                        .iter()
                        .take(self.reviews_visible)
                        .enumerate()
                        .map(|(i, r)| self.review_card(r, format!("store-{i}"), false, cx)),
                )
                .when(matching.len() > self.reviews_visible, |d| {
                    d.child(
                        text_button("more-reviews", "Show more reviews")
                            .self_start()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reviews_visible += 4;
                                cx.notify();
                            })),
                    )
                });
        }
        content
    }
    fn review_column(&self, page: &Page, recent: bool, cx: &mut Context<Self>) -> Div {
        let index = usize::from(recent);
        let visible = if recent {
            self.review_state.recent_visible
        } else {
            self.reviews_visible
        };
        let error = self.review_state.errors[index]
            .clone()
            .or_else(|| page.error.clone());
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rems(0.9))
            .when(recent, |d| d.child(heading("Recently posted")))
            .when(!recent && !page.summary.is_empty(), |d| {
                d.child(muted(page.summary.clone()))
            })
            .when(self.review_state.loading[index], |d| {
                d.child(
                    pill()
                        .self_start()
                        .p(rems(0.8))
                        .gap(rems(0.6))
                        .child(spinner(
                            if recent {
                                "recent-reviews-loading"
                            } else {
                                "main-reviews-loading"
                            },
                            1.2,
                        ))
                        .child("Loading reviews…"),
                )
            })
            .when_some(error.clone(), |d, error| {
                d.child(
                    panel().child(muted(error)).child(
                        text_button(("retry-reviews", index), "Retry")
                            .self_start()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.request_reviews(recent, !page_is_empty(this, recent), cx)
                            })),
                    ),
                )
            })
            .when(
                page.reviews.is_empty() && !self.review_state.loading[index] && error.is_none(),
                |d| d.child(panel().child(muted("No written reviews match these filters."))),
            )
            .children(
                page.reviews.iter().take(visible).enumerate().map(|(i, r)| {
                    self.review_card(r, format!("{}-{i}-{}", index, r.id), recent, cx)
                }),
            )
            .when(
                !self.review_state.loading[index]
                    && (page.reviews.len() > visible || page.cursor.is_some()),
                |d| {
                    d.child(
                        text_button(("more-steam-reviews", index), "Show more reviews")
                            .self_start()
                            .child(icon("more"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let count = this
                                    .review_state
                                    .pages
                                    .as_ref()
                                    .or_else(|| this.data.as_ref()?.steam_reviews.as_ref())
                                    .map(|p| {
                                        if recent {
                                            p.recent.reviews.len()
                                        } else {
                                            p.main.reviews.len()
                                        }
                                    })
                                    .unwrap_or(0);
                                let visible = if recent {
                                    &mut this.review_state.recent_visible
                                } else {
                                    &mut this.reviews_visible
                                };
                                if *visible < count {
                                    *visible += 4;
                                    cx.notify();
                                } else {
                                    this.request_reviews(recent, true, cx);
                                }
                            })),
                    )
                },
            )
    }
    fn review_card(
        &self,
        review: &Review,
        key: String,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let expanded = self.review_state.expanded.contains(&key);
        let mut footer = vec![];
        if !review.date.is_empty() {
            footer.push(format!("Posted {}", review.date));
        }
        if let Some(hours) = review.hours {
            footer.push(format!("{hours:.1} hours played"));
        }
        if let Some(hours) = review.hours_at_review {
            footer.push(format!("{hours:.1} hours at review"));
        }
        if !review.platform.is_empty() {
            footer.push(review.platform.clone());
        }
        let author = review.author.clone();
        let url = (!review.steam_id.is_empty())
            .then(|| format!("https://steamcommunity.com/profiles/{}/", review.steam_id));
        let full_text = if review.title.is_empty() {
            review.text.clone()
        } else {
            format!("{}\n{}", review.title, review.text)
        };
        let limit = if compact { 220 } else { 650 };
        let long = full_text.graphemes(true).count() > limit;
        let preview = if long && !expanded {
            format!(
                "{}…",
                full_text
                    .graphemes(true)
                    .take(limit)
                    .collect::<String>()
                    .trim_end()
            )
        } else {
            full_text
        };
        let body = review_paragraph(preview).max_w_full().flex_none();
        panel()
            .id(SharedString::from(format!("review-card-{key}")))
            .w_full()
            .min_w_0()
            .max_w_full()
            .flex_none()
            .overflow_hidden()
            .p(rems(1.1))
            .gap(rems(0.65))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(rems(0.65))
                    .when_some(review.recommended(), |d, good| {
                        d.child(
                            icon(if good { "thumb-up" } else { "thumb-down" })
                                .flex_none()
                                .text_color(rgb(if good { 0x65d69a } else { 0xf4787b })),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("author-{key}")))
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if author.is_empty() {
                                "Player".into()
                            } else {
                                author
                            })
                            .when_some(url, |d, url| {
                                d.cursor_pointer().on_click(move |_, _, cx| {
                                    if std::env::var_os("EXPOSED_GAMES_DISABLE_ACTIONS").is_none() {
                                        cx.open_url(&url);
                                    }
                                })
                            }),
                    ),
            )
            .when(
                !review.rating.is_empty()
                    && !matches!(
                        review.rating.to_ascii_lowercase().as_str(),
                        "recommended" | "not recommended"
                    ),
                |d| d.child(review_note(review.rating.clone())),
            )
            .child(body)
            .when(long, |d| {
                d.child(
                    text_button(
                        SharedString::from(format!("expand-{key}")),
                        if expanded { "Read less" } else { "Read more" },
                    )
                    .self_start()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.review_state.expanded.remove(&key) {
                            this.review_state.expanded.insert(key.clone());
                        }
                        cx.notify();
                    })),
                )
            })
            .when(!footer.is_empty(), |d| {
                d.child(review_note(footer.join(" · ")))
            })
            .when(review.helpful.is_some(), |d| {
                d.child(review_note(format!(
                    "{} found this helpful · {} funny",
                    review.helpful.unwrap_or(0),
                    review.funny.unwrap_or(0)
                )))
            })
            .when(review.received_free, |d| {
                d.child(review_note("Product received for free"))
            })
    }
}
// Every review child gets the card's available width before text measurement.
// This keeps metadata wrapping with the body instead of sizing to its contents.
fn review_note(text: impl Into<SharedString>) -> Div {
    muted(text).w_full().min_w_0().max_w_full().flex_none()
}

fn page_is_empty(this: &DetailsPage, recent: bool) -> bool {
    this.review_state
        .pages
        .as_ref()
        .or_else(|| this.data.as_ref()?.steam_reviews.as_ref())
        .is_none_or(|p| {
            if recent {
                p.recent.reviews.is_empty()
            } else {
                p.main.reviews.is_empty()
            }
        })
}
