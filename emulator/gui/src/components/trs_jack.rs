//! Quarter-inch TRS output/input jack component.
use gpui::{
    App, IntoElement, PathBuilder, Pixels, Point, RenderOnce, Window, canvas, div, point,
    prelude::*, px, rgb,
};

#[derive(IntoElement)]
pub struct TrsJack {
    pub diameter: f32,
}

impl TrsJack {
    pub fn new(diameter: f32) -> Self {
        Self { diameter }
    }
}

impl RenderOnce for TrsJack {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let diameter = self.diameter;
        div().size(px(diameter)).flex_shrink_0().child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let d = bounds.size.width.as_f32().min(bounds.size.height.as_f32());
                    let center =
                        point(bounds.origin.x + px(d * 0.5), bounds.origin.y + px(d * 0.5));
                    circle(window, center, d * 0.49, 0x16191b);
                    polygon(window, center, d * 0.48, 6, 0xb5babd);
                    polygon(window, center, d * 0.435, 6, 0x777c80);
                    for (radius, color) in [
                        (0.40, 0xd4d8db),
                        (0.365, 0x363a3d),
                        (0.335, 0x969c9f),
                        (0.295, 0x2b2d30),
                        (0.26, 0x5b6064),
                        (0.225, 0x090a0b),
                    ] {
                        circle(window, center, d * radius, color);
                    }
                },
            )
            .size_full(),
        )
    }
}

fn circle(window: &mut Window, center: Point<Pixels>, radius: f32, color: u32) {
    let mut path = PathBuilder::fill();
    path.move_to(point(center.x + px(radius), center.y));
    path.arc_to(
        point(px(radius), px(radius)),
        px(0.0),
        false,
        true,
        point(center.x - px(radius), center.y),
    );
    path.arc_to(
        point(px(radius), px(radius)),
        px(0.0),
        false,
        true,
        point(center.x + px(radius), center.y),
    );
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color));
    }
}

fn polygon(window: &mut Window, center: Point<Pixels>, radius: f32, sides: usize, color: u32) {
    let mut path = PathBuilder::fill();
    for side in 0..sides {
        let angle = side as f32 / sides as f32 * std::f32::consts::TAU;
        let vertex = point(
            center.x + px(radius * angle.cos()),
            center.y + px(radius * angle.sin()),
        );
        if side == 0 {
            path.move_to(vertex);
        } else {
            path.line_to(vertex);
        }
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color));
    }
}
