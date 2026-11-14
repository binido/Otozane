//! Client-side window chrome: on Linux the window has no system decorations,
//! so the title bar drag area, the window buttons and edge resizing live here.
//! macOS keeps its system frame and uses only the drag area.

use crate::theme;
use egui::viewport::ResizeDirection;
use egui::{Context, CursorIcon, PointerButton, Rect, Sense, Stroke, Ui, Vec2, ViewportCommand};

/// Height of the title bar strip at the top of the window.
pub const HEIGHT: f32 = 64.0;
/// Extra top space in the sidebar on macOS, below the traffic lights.
pub const TRAFFIC_LIGHTS: f32 = 20.0;

fn maximized(ctx: &Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Drag `rect` to move the window; double click toggles maximize.
/// Call before adding the widgets on top of it, so they keep their clicks.
pub fn drag_area(ui: &mut Ui, rect: Rect) {
    let resp = ui.interact(
        rect,
        ui.scope_id().with("titlebar"),
        Sense::click_and_drag(),
    );
    if resp.double_clicked() {
        let max = maximized(ui.ctx());
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
    } else if resp.drag_started_by(PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// libadwaita-style round window buttons. Expects a right-to-left layout.
pub fn buttons(ui: &mut Ui) {
    ui.spacing_mut().item_spacing.x = 10.0;
    let ctx = ui.ctx().clone();
    if button(ui, Glyph::Close, "Close").clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let max = maximized(&ctx);
    let (glyph, tip) = if max {
        (Glyph::Restore, "Restore")
    } else {
        (Glyph::Maximize, "Maximize")
    };
    if button(ui, glyph, tip).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    if button(ui, Glyph::Minimize, "Minimize").clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

fn button(ui: &mut Ui, glyph: Glyph, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    let p = theme::current(ui.ctx());
    let w = &ui.visuals().widgets;
    let fill = if resp.is_pointer_button_down_on() {
        w.active.bg_fill
    } else if resp.hovered() {
        w.hovered.bg_fill
    } else {
        p.raised
    };
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 12.0, fill);
    let c = rect.center();
    let s = Stroke::new(1.4, p.text);
    let h = 4.0;
    match glyph {
        Glyph::Minimize => {
            painter.hline((c.x - h)..=(c.x + h), c.y + 3.0, s);
        }
        Glyph::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(2.0 * h)),
                1.0,
                s,
                egui::StrokeKind::Middle,
            );
        }
        Glyph::Restore => {
            let r = Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(6.5));
            painter.rect_stroke(r, 1.0, s, egui::StrokeKind::Middle);
            painter.hline((r.left() + 2.0)..=(r.right() + 2.0), r.top() - 2.0, s);
            painter.vline(r.right() + 2.0, (r.top() - 2.0)..=(r.bottom() - 2.0), s);
        }
        Glyph::Close => {
            painter.line_segment([c - Vec2::splat(h), c + Vec2::splat(h)], s);
            painter.line_segment([c + Vec2::new(-h, h), c + Vec2::new(h, -h)], s);
        }
    }
    resp
}

/// Resizes the window from its edges: sets the cursor near an edge and hands
/// the drag to the compositor on press.
pub fn resize_edges(ctx: &Context) {
    const EDGE: f32 = 6.0;
    let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    if maximized(ctx) || fullscreen {
        return;
    }
    let Some(pos) = ctx.pointer_hover_pos() else {
        return;
    };
    let r = ctx.content_rect();
    let (l, rt) = (pos.x - r.left() < EDGE, r.right() - pos.x < EDGE);
    let (t, b) = (pos.y - r.top() < EDGE, r.bottom() - pos.y < EDGE);
    use ResizeDirection as D;
    let (dir, cursor) = match (l, rt, t, b) {
        (true, _, true, _) => (D::NorthWest, CursorIcon::ResizeNorthWest),
        (_, true, true, _) => (D::NorthEast, CursorIcon::ResizeNorthEast),
        (true, _, _, true) => (D::SouthWest, CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (D::SouthEast, CursorIcon::ResizeSouthEast),
        (true, ..) => (D::West, CursorIcon::ResizeWest),
        (_, true, ..) => (D::East, CursorIcon::ResizeEast),
        (_, _, true, _) => (D::North, CursorIcon::ResizeNorth),
        (.., true) => (D::South, CursorIcon::ResizeSouth),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
}
