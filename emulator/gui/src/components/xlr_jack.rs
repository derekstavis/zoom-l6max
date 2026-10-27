//! XLR/TRS combo input jack component.
use gpui::{
    App, IntoElement, PathBuilder, RenderOnce, Window, canvas, div, point, prelude::*, px, rgb,
};

#[derive(IntoElement)]
pub struct XlrJack {
    pub diameter: f32,
}

impl XlrJack {
    pub fn new(diameter: f32) -> Self {
        Self { diameter }
    }
}

impl RenderOnce for XlrJack {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let size = self.diameter;
        let mut face = div().relative().size(px(size)).flex_shrink_0().child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let d = bounds.size.width.as_f32().min(bounds.size.height.as_f32());
                    let center =
                        point(bounds.origin.x + px(d * 0.5), bounds.origin.y + px(d * 0.5));
                    for (radius, color) in [
                        (0.495, 0x141516),
                        (0.470, 0x393b3d),
                        (0.445, 0x111213),
                        (0.397, 0x080909),
                        (0.365, 0x454648),
                        (0.342, 0x3a3c3e),
                        (0.145, 0x050606),
                    ] {
                        circle(window, center.x, center.y, d * radius, color);
                    }
                    for (dx, dy) in [(-0.165, 0.0), (0.165, 0.0), (0.0, 0.165)] {
                        circle(
                            window,
                            center.x + px(d * dx),
                            center.y + px(d * dy),
                            d * 0.069,
                            0x020303,
                        );
                    }
                    let mut notch = PathBuilder::fill();
                    for (i, (x, y)) in [
                        (0.44, 0.024),
                        (0.56, 0.024),
                        (0.56, 0.09),
                        (0.53, 0.115),
                        (0.47, 0.115),
                        (0.44, 0.09),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let p = point(bounds.origin.x + px(d * x), bounds.origin.y + px(d * y));
                        if i == 0 {
                            notch.move_to(p);
                        } else {
                            notch.line_to(p);
                        }
                    }
                    notch.close();
                    if let Ok(path) = notch.build() {
                        window.paint_path(path, rgb(0x080909));
                    }
                },
            )
            .size_full(),
        );
        for (x, y, number) in [(0.27, 0.25, "2"), (0.27, 0.61, "3"), (0.70, 0.52, "1")] {
            face = face.child(
                div()
                    .absolute()
                    .left(px(size * x))
                    .top(px(size * y))
                    .text_size(px(size * 0.09))
                    .line_height(px(size * 0.11))
                    .text_color(rgb(0x616367))
                    .child(number),
            );
        }
        face
    }
}

fn circle(window: &mut Window, x: gpui::Pixels, y: gpui::Pixels, radius: f32, color: u32) {
    let center = point(x, y);
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
