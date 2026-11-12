use super::*;
use exposed_games_backend::player_stats::{Achievement, PlayerStats};

fn hours(seconds: u64) -> String {
    let n = seconds as f64 / 3600.;
    if n.fract() < 0.05 {
        format!("Played {n:.0} hours")
    } else {
        format!("Played {n:.1} hours")
    }
}
fn stats_panel() -> Div {
    panel()
        .flex_1()
        .min_w(rems(15.))
        .max_w(rems(26.))
        .p(rems(0.75))
        .gap(rems(0.5))
        .items_stretch()
        .justify_between()
}
fn preview_slots(width: f32, rem: f32) -> usize {
    (((width - 1.).max(0.) / rem + 0.4) / 3.1).floor().max(1.) as usize
}
fn status(a: &Achievement) -> String {
    let mut parts = Vec::new();
    if a.unlocked {
        if let Some(date) = a.unlocked_at {
            let date = exposed_games_backend::details::display_date(&date.to_string());
            if !date.is_empty() {
                parts.push(date);
            }
        }
    } else {
        parts.push("Locked".to_owned());
        if a.progress.is_some() {
            parts.push(format!("{:.0}% complete", a.completion() * 100.));
        }
    }
    if let Some(rarity) = a.rarity {
        parts.push(format!("{rarity:.1}% of players"));
    }
    parts.join(" · ")
}
fn rarity_line(a: &Achievement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(rems(0.35))
        .when(a.unlocked && a.rarity.is_some_and(|n| n < 10.), |d| {
            d.child(icon("rare").text_color(rgb(0xf3ce68)).size(rems(1.125)))
        })
        .child(muted(status(a)))
}
fn achievement_image(a: &Achievement, image: Option<Arc<RenderImage>>, size: f32) -> Div {
    let image = div()
        .size_full()
        .flex_none()
        .rounded(rems(0.6))
        .overflow_hidden()
        .bg(rgba(0xffffff12))
        .flex()
        .items_center()
        .justify_center()
        .when_some(image.clone(), |d, image| {
            d.child(
                img(image)
                    .size_full()
                    .rounded(rems(0.6))
                    .object_fit(ObjectFit::Contain),
            )
        })
        .when(image.is_none() && a.icon.is_empty(), |d| {
            d.child(
                icon(if a.unlocked { "star" } else { "star-empty" })
                    .size(rems(size * 0.55))
                    .text_color(rgb(0xbccfc7)),
            )
        })
        .opacity(if a.unlocked { 1. } else { 0.55 });
    div()
        .relative()
        .size(rems(size))
        .flex_none()
        .rounded(rems(0.6))
        .overflow_hidden()
        .child(image)
        .when(a.unlocked && a.rarity.is_some_and(|n| n < 10.), |d| {
            d.child(div().absolute().inset_0().with_animation(
                SharedString::from(format!("rare-glare-{}", a.id)),
                Animation::new(millis(2400)).repeat(),
                |d, progress| {
                    d.child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, (), window, _| {
                                paint_achievement_glare(bounds, progress, window);
                            },
                        )
                        .size_full(),
                    )
                },
            ))
        })
}
fn paint_achievement_glare(bounds: Bounds<Pixels>, progress: f32, window: &mut Window) {
    if progress >= 0.32 || bounds.size.width <= px(0.) {
        return;
    }
    let center = (progress / 0.32) * 2.1 - 0.55;
    let radius = f32::from(window.rem_size() * 0.6 / bounds.size.width).min(0.5);
    let mut outline = Vec::with_capacity(36);
    for (x, y, angle) in [
        (radius, radius, std::f32::consts::PI),
        (1. - radius, radius, -std::f32::consts::FRAC_PI_2),
        (1. - radius, 1. - radius, 0.),
        (radius, 1. - radius, std::f32::consts::FRAC_PI_2),
    ] {
        for step in 0..=8 {
            let angle = angle + step as f32 * std::f32::consts::PI / 16.;
            outline.push((x + radius * angle.cos(), y + radius * angle.sin()));
        }
    }
    for (side, from, to, colors) in [
        (-1., center - 0.325, center, [0xffdc7a00, 0xffe6a350]),
        (1., center, center + 0.325, [0xffe6a350, 0xffdc7a00]),
    ] {
        let distance =
            |p: (f32, f32)| side * ((p.0 + p.1 - 1.) / std::f32::consts::SQRT_2 + 0.5 - center);
        let mut polygon = Vec::with_capacity(38);
        let mut previous = *outline.last().unwrap();
        let mut previous_distance = distance(previous);
        for &current in &outline {
            let current_distance = distance(current);
            if (previous_distance >= 0.) != (current_distance >= 0.) {
                let t = previous_distance / (previous_distance - current_distance);
                polygon.push((
                    previous.0 + t * (current.0 - previous.0),
                    previous.1 + t * (current.1 - previous.1),
                ));
            }
            if current_distance >= 0. {
                polygon.push(current);
            }
            previous = current;
            previous_distance = current_distance;
        }
        if polygon.len() < 3 {
            continue;
        }
        let position = |p: (f32, f32)| {
            bounds.origin + point(bounds.size.width * p.0, bounds.size.height * p.1)
        };
        let mut path = PathBuilder::fill();
        path.move_to(position(polygon[0]));
        for &p in &polygon[1..] {
            path.line_to(position(p));
        }
        path.close();
        if let Ok(mut path) = path.build() {
            path.bounds = bounds;
            window.paint_path(
                path,
                linear_gradient(
                    135.,
                    linear_color_stop(rgba(colors[0]), from),
                    linear_color_stop(rgba(colors[1]), to),
                ),
            );
        }
    }
}

struct AchievementTooltip {
    achievement: Achievement,
    image: Option<Arc<RenderImage>>,
}
impl Render for AchievementTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let a = &self.achievement;
        panel()
            .max_w(rems(26.))
            .p(rems(1.))
            .gap(rems(0.65))
            .text_size(rems(1.125))
            .text_color(rgb(0xf0f8f3))
            .bg(rgba(0x151515f2))
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rems(0.75))
                    .child(achievement_image(a, self.image.clone(), 2.7))
                    .child(paragraph(a.name.clone()).font_weight(FontWeight::SEMIBOLD)),
            )
            .when_some(a.description_text(), |d, text| {
                d.child(paragraph(text.to_owned()))
            })
            .child(rarity_line(a))
    }
}
impl DetailsPage {
    fn set_stats(&mut self, stats: PlayerStats, cx: &mut Context<Self>) {
        let top = self.achievement_rows.logical_scroll_top();
        let anchor = self
            .stats
            .as_ref()
            .and_then(|s| {
                self.achievement_order
                    .get(top.item_ix)
                    .and_then(|&i| s.achievements.get(i))
            })
            .map(|a| a.id.clone());
        self.achievement_order = stats.by_progress();
        let hidden: Vec<_> = self
            .achievement_order
            .iter()
            .copied()
            .filter(|&i| stats.achievements[i].concealed())
            .collect();
        self.achievement_order
            .retain(|&i| !stats.achievements[i].concealed());
        if !hidden.is_empty() {
            self.achievement_order.push(usize::MAX);
            if self.hidden_achievements_expanded {
                self.achievement_order.extend(hidden);
            }
        }
        self.achievement_rows
            .reset_with_uniform_height(self.achievement_order.len(), px(self.backend.zoom() * 9.));
        if let Some(index) = anchor.and_then(|id| {
            self.achievement_order
                .iter()
                .position(|&i| stats.achievements.get(i).is_some_and(|a| a.id == id))
        }) {
            self.achievement_rows.scroll_to(ListOffset {
                item_ix: index,
                offset_in_item: top.offset_in_item,
            });
        }
        self.stats = Some(Arc::new(stats));
        cx.notify();
    }
    pub(super) fn load_stats(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let game = self.game.clone();
        self.stats_task = Some(cx.spawn(async move |this, cx| {
            let cached = cx
                .background_executor()
                .spawn({
                    let backend = backend.clone();
                    let game = game.clone();
                    async move {
                        let stats = backend.player_stats(&game, false);
                        backend.warm_achievement_icons(&stats);
                        stats
                    }
                })
                .await;
            if this
                .update(cx, |this, cx| {
                    this.set_stats(cached, cx);
                    this.stats_loading = false;
                })
                .is_err()
            {
                return;
            }
            let stats = cx
                .background_executor()
                .spawn(async move {
                    let stats = backend.player_stats(&game, true);
                    backend.warm_achievement_icons(&stats);
                    stats
                })
                .await;
            this.update(cx, |this, cx| this.set_stats(stats, cx)).ok();
        }));
    }
    pub(super) fn close_achievements(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.achievements_open = false;
        self.achievement_wheel.reset(0.);
        window.focus(&self.return_focus, cx);
        cx.notify();
    }
    pub(super) fn stats_block(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.stats_loading {
            return Some(
                stats_panel()
                    .h(rems(8.7))
                    .bg(rgba(0xffffff20))
                    .into_any_element(),
            );
        }
        let stats = self.stats.clone()?;
        if stats.achievements.is_empty() {
            return stats.playtime_seconds.map(|seconds| {
                bubble()
                    .self_start()
                    .child(hours(seconds))
                    .into_any_element()
            });
        }
        let mut block = stats_panel();
        if let Some(seconds) = stats.playtime_seconds {
            block = block.child(
                div()
                    .w_full()
                    .text_left()
                    .whitespace_nowrap()
                    .child(hours(seconds)),
            );
        }
        let featured = stats.featured();
        if let Some(featured) = featured {
            let a = &stats.achievements[featured];
            block = block.child(
                self.preview_achievement(a, featured, cx)
                    .w_full()
                    .gap(rems(0.65))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rems(0.05))
                            .child(
                                paragraph(a.name.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_clamp(2),
                            )
                            .when_some(a.description_text(), |d, text| {
                                d.child(paragraph(text.to_owned()).line_clamp(2))
                            }),
                    ),
            );
        } else {
            block = block.child(muted(format!(
                "{} hidden achievements",
                stats.achievements.len()
            )));
        }
        let mut icons = div().flex().flex_none().items_center().gap(rems(0.4));
        for index in stats
            .highlights()
            .into_iter()
            .filter(|&i| Some(i) != featured)
            .take(self.achievement_preview_slots.saturating_sub(1))
        {
            icons = icons.child(
                self.preview_achievement(&stats.achievements[index], index, cx)
                    .flex_none(),
            );
        }
        let owner = cx.weak_entity();
        let tiles = div()
            .id("achievement-preview-row")
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(rems(0.4))
            .on_prepaint(move |paint, window, cx| {
                let count = preview_slots(
                    f32::from(paint.bounds.size.width),
                    f32::from(window.rem_size()),
                );
                owner
                    .update(cx, |this, cx| {
                        if this.achievement_preview_slots != count {
                            this.achievement_preview_slots = count;
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .child(icons)
            .child(
                div()
                    .id("more-achievements")
                    .size(rems(2.7))
                    .flex_none()
                    .rounded(rems(0.6))
                    .bg(rgba(0xffffff12))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(rgba(0xffffff25)))
                    .child(icon("achievement-more").size(rems(1.35)))
                    .when(!self.achievements_open, |d| {
                        d.tooltip(|_, cx| {
                            cx.new(|_| DetailTooltip("All achievements".into())).into()
                        })
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.filter_menu = None;
                        this.achievements_open = true;
                        this.achievement_wheel.reset(-f32::from(
                            this.achievement_rows.scroll_px_offset_for_scrollbar().y,
                        ) as f64);
                        this.wheel.reset(-f32::from(this.scroll.offset().y) as f64);
                        window.focus(&this.achievement_focus, cx);
                        cx.notify();
                    })),
            );
        Some(block.child(tiles).into_any_element())
    }
    fn preview_achievement(
        &mut self,
        a: &Achievement,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.image(&a.icon, cx);
        let a = a.clone();
        let image = self.images.get(&a.icon).cloned();
        div()
            .id(("achievement-preview", index))
            .flex()
            .items_start()
            .min_w_0()
            .child(achievement_image(&a, image.clone(), 2.7))
            .tooltip_show_delay(millis(250))
            .tooltip(move |_, cx| {
                cx.new(|_| AchievementTooltip {
                    achievement: a.clone(),
                    image: image.clone(),
                })
                .into()
            })
    }
    fn achievement_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(stats) = self.stats.clone() else {
            return div().into_any_element();
        };
        if self.achievement_order.get(index) == Some(&usize::MAX) {
            let count = stats.achievements.iter().filter(|a| a.concealed()).count();
            return div()
                .px(rems(1.25))
                .pb(rems(0.75))
                .child(
                    text_button(
                        "hidden-achievements",
                        format!("{count} hidden achievements"),
                    )
                    .child(icon(if self.hidden_achievements_expanded {
                        "collapse"
                    } else {
                        "next"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.hidden_achievements_expanded = !this.hidden_achievements_expanded;
                        this.set_stats((*stats).clone(), cx);
                    })),
                )
                .into_any_element();
        }
        let Some(a) = self
            .achievement_order
            .get(index)
            .and_then(|&i| stats.achievements.get(i))
        else {
            return div().into_any_element();
        };
        self.image(&a.icon, cx);
        div()
            .id(("achievement-row", index))
            .px(rems(1.25))
            .pb(rems(0.75))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rems(1.))
                    .p(rems(1.))
                    .rounded(rems(1.))
                    .bg(rgba(0xffffff0c))
                    .child(achievement_image(a, self.images.get(&a.icon).cloned(), 3.5))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rems(0.15))
                            .child(paragraph(a.name.clone()).font_weight(FontWeight::SEMIBOLD))
                            .when_some(a.description_text(), |d, text| {
                                d.child(review_paragraph(text.to_owned()))
                            })
                            .child(rarity_line(a))
                            .when(!a.unlocked && a.progress.is_some(), |d| {
                                d.child(
                                    div()
                                        .h(rems(0.3))
                                        .w_full()
                                        .rounded_full()
                                        .overflow_hidden()
                                        .bg(rgba(0xffffff18))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(a.completion() as f32))
                                                .bg(rgb(0x91d8b2)),
                                        ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn scroll_achievements(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .achievement_rows
            .viewport_bounds()
            .contains(&event.position)
        {
            cx.stop_propagation();
            return;
        }
        let position = -f32::from(self.achievement_rows.scroll_px_offset_for_scrollbar().y) as f64;
        let ScrollDelta::Lines(lines) = event.delta else {
            self.achievement_wheel.reset(position);
            return;
        };
        if lines.y == 0. || event.modifiers.control {
            return;
        }
        cx.stop_propagation();
        if !self.achievement_wheel.active() {
            self.achievement_wheel.reset(position);
        }
        let max = f32::from(self.achievement_rows.max_offset_for_scrollbar().y) as f64;
        self.achievement_wheel.push(
            -lines.y as f64 * f32::from(window.rem_size()) as f64 * 2.5,
            Instant::now(),
            max,
        );
        self.animate_achievements(window, cx);
    }
    fn animate_achievements(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.achievements_open
            || !self.achievement_wheel.active()
            || self.achievement_frame_pending
        {
            return;
        }
        self.achievement_frame_pending = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.achievement_frame_pending = false;
            if !this.achievements_open || !this.achievement_wheel.active() {
                return;
            }
            let position = this.achievement_wheel.sample(Instant::now());
            let offset = -window.pixel_snap(px(position as f32));
            this.achievement_rows
                .set_offset_from_scrollbar(point(px(0.), offset));
            cx.notify();
            this.animate_achievements(window, cx);
        });
    }
    pub(super) fn achievements_popup(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.achievements_open {
            return div().into_any_element();
        }
        let Some(stats) = self.stats.clone() else {
            return div().into_any_element();
        };
        let owner = cx.weak_entity();
        let total = stats.total.unwrap_or(stats.achievements.len());
        let unlocked = stats
            .unlocked_count
            .unwrap_or_else(|| stats.achievements.iter().filter(|a| a.unlocked).count())
            .min(total);
        let rows = list(self.achievement_rows.clone(), move |row, _, cx| {
            owner
                .update(cx, |this, cx| this.achievement_row(row, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .size_full();
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x00000070))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_achievements(window, cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                panel()
                    .id("achievements-popup")
                    .track_focus(&self.achievement_focus)
                    .w(rems(47.))
                    .max_w(relative(0.9))
                    .h(relative(0.78))
                    .max_h(rems(48.))
                    .p_0()
                    .gap(rems(0.75))
                    .bg(rgba(0x181818f2))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_action(cx.listener(|this, _: &crate::ClearSearch, window, cx| {
                        this.close_achievements(window, cx);
                        cx.stop_propagation();
                    }))
                    .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                        if e.keystroke.key == "escape" {
                            this.close_achievements(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(rems(1.5))
                            .pt(rems(1.25))
                            .flex_none()
                            .child(heading("Achievements"))
                            .child(
                                action_button("close-achievements", "close", "Close achievements")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_achievements(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .px(rems(1.5))
                            .pb(rems(0.25))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(rems(0.6))
                            .child(muted(format!(
                                "{unlocked} / {total} unlocked ({:.0}%)",
                                if total > 0 {
                                    unlocked as f64 / total as f64 * 100.
                                } else {
                                    0.
                                }
                            )))
                            .child(
                                div()
                                    .h(rems(0.4))
                                    .w_full()
                                    .rounded_full()
                                    .overflow_hidden()
                                    .bg(rgba(0xffffff20))
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .w(relative(if total > 0 {
                                                unlocked as f32 / total as f32
                                            } else {
                                                0.
                                            }))
                                            .bg(rgb(0xf3ce68)),
                                    ),
                            ),
                    )
                    .when(stats.partial, |d| {
                        d.child(
                            muted(format!(
                                "Cached achievements · {} of {}",
                                stats.achievements.len(),
                                stats.total.unwrap_or(stats.achievements.len())
                            ))
                            .px(rems(1.5)),
                        )
                    })
                    .child(div().flex_1().min_h_0().child(rows)),
            )
            .into_any_element()
    }
}
