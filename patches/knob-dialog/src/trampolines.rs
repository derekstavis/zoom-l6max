use core::arch::global_asm;
global_asm!(include_str!("trampolines.s"), options(raw));
unsafe extern "C" {
    fn original_notification_pop();
    fn original_notification_tick();
    fn original_notification_show();
    fn original_overlays_clear();
    fn original_busy_hide();
    fn original_busy_show();
    fn original_notification_pause();
    fn original_notification_remove();
    fn original_notification_current();
    fn original_notification_redraw_request();
    fn original_busy_tick();
    fn original_notification_draw();
    fn original_notification_bind();
    fn original_display_scene_refresh();
    fn original_widget_polarity();
    fn original_widget_y();
    fn original_widget_resource();
    fn original_localized_text();
    fn original_text_layout();
    fn original_text_measure();
    fn original_text_copy();
    fn original_text_scroll();
    fn original_scene_clear();
    fn original_callback_unregister();
    fn original_window_push();
    fn original_notification_active();
    fn original_background_draw();
}
pub fn notification_pop() -> usize {
    original_notification_pop as *const () as usize
}
pub fn notification_tick() -> usize {
    original_notification_tick as *const () as usize
}
pub fn notification_show() -> usize {
    original_notification_show as *const () as usize
}
pub fn overlays_clear() -> usize {
    original_overlays_clear as *const () as usize
}
pub fn busy_hide() -> usize {
    original_busy_hide as *const () as usize
}
pub fn busy_show() -> usize {
    original_busy_show as *const () as usize
}
pub fn notification_pause() -> usize {
    original_notification_pause as *const () as usize
}
pub fn notification_remove() -> usize {
    original_notification_remove as *const () as usize
}
pub fn notification_current() -> usize {
    original_notification_current as *const () as usize
}
pub fn notification_redraw_request() -> usize {
    original_notification_redraw_request as *const () as usize
}
pub fn busy_tick() -> usize {
    original_busy_tick as *const () as usize
}
pub fn notification_draw() -> usize {
    original_notification_draw as *const () as usize
}
pub fn notification_bind() -> usize {
    original_notification_bind as *const () as usize
}
pub fn display_scene_refresh() -> usize {
    original_display_scene_refresh as *const () as usize
}
pub fn widget_polarity() -> usize {
    original_widget_polarity as *const () as usize
}
pub fn widget_y() -> usize {
    original_widget_y as *const () as usize
}
pub fn widget_resource() -> usize {
    original_widget_resource as *const () as usize
}
pub fn localized_text() -> usize {
    original_localized_text as *const () as usize
}
pub fn text_layout() -> usize {
    original_text_layout as *const () as usize
}
pub fn text_measure() -> usize {
    original_text_measure as *const () as usize
}
pub fn text_copy() -> usize {
    original_text_copy as *const () as usize
}
pub fn text_scroll() -> usize {
    original_text_scroll as *const () as usize
}
pub fn scene_clear() -> usize {
    original_scene_clear as *const () as usize
}
pub fn callback_unregister() -> usize {
    original_callback_unregister as *const () as usize
}
pub fn window_push() -> usize {
    original_window_push as *const () as usize
}
pub fn notification_active() -> usize {
    original_notification_active as *const () as usize
}
pub fn background_draw() -> usize {
    original_background_draw as *const () as usize
}
