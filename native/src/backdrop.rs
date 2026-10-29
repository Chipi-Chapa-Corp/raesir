use gpui::{prelude::*, *};
use std::{cell::Cell, rc::Rc};

pub struct Backdrop {
    from: [Hsla; 2],
    target: [Hsla; 2],
    displayed: Rc<Cell<[Hsla; 2]>>,
    revision: usize,
}

fn colors(index: usize) -> [Hsla; 2] {
    let (a, b) = crate::palette(index);
    [rgb_to_hsla(rgb(a)), rgb_to_hsla(rgb(b))]
}

impl Default for Backdrop {
    fn default() -> Self {
        Self {
            from: colors(0),
            target: colors(0),
            displayed: Rc::new(Cell::new(colors(0))),
            revision: 0,
        }
    }
}

impl Backdrop {
    pub fn retarget(&mut self, index: usize) -> bool {
        let target = colors(index);
        if target == self.target {
            return false;
        }
        self.from = self.displayed.get();
        self.target = target;
        self.revision += 1;
        true
    }
}

impl Render for Backdrop {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let [a, b] = self.from;
        let [c, d] = self.target;
        let background = div().absolute().inset_0().size_full();
        if self.from == self.target {
            return background
                .bg(linear_gradient(
                    130.,
                    linear_color_stop(a, 0.),
                    linear_color_stop(b, 1.),
                ))
                .into_any_element();
        }
        let displayed = self.displayed.clone();
        background
            .with_animation(
                ("gradient", self.revision),
                Animation::new(millis(500)).with_easing(ease_in_out),
                move |element, progress| {
                    let [a, b] = [a.lerp(&c, progress), b.lerp(&d, progress)];
                    displayed.set([a, b]);
                    element.bg(linear_gradient(
                        130.,
                        linear_color_stop(a, 0.),
                        linear_color_stop(b, 1.),
                    ))
                },
            )
            .into_any_element()
    }
}
