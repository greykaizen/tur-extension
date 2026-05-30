// crates/host/src/overlay/wndproc.rs

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM, TRUE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, EndPaint, SetWindowRgn, PAINTSTRUCT,
    RGN_OR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::overlay::canvas;
use crate::overlay::menu::show_quality_menu;
use crate::overlay::render_frame;
use crate::overlay::types::*;
use crate::overlay::write_response;

/// WndProc function for the canvas overlay window.
pub unsafe extern "system" fn canvas_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => return LRESULT(1),

        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            render_frame();
            return LRESULT(0);
        }

        WM_ERASEBKGND => return LRESULT(1),

        WM_NCHITTEST => {
            return LRESULT(HTCLIENT as isize);
        }

        WM_LBUTTONDOWN => {
            let cx = (lparam.0 & 0xFFFF) as i16 as i32;
            let cy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut screen_pt = POINT { x: cx, y: cy };
            let _ = ClientToScreen(hwnd, &mut screen_pt);

            let hit = {
                let g = canvas().lock().unwrap();
                if let Some(ref state) = *g {
                    if let Some(ref gpu) = state.gpu {
                        let dpr = state.dpr as f32;
                        let hud_w = (gpu.hud_width * dpr) as i32;
                        let hud_h = (HUD_H * dpr) as i32;
                        let hud_gap = (HUD_GAP * dpr) as i32;
                        let pill_x = (PILL_START_X * dpr) as i32;
                        let x_off =
                            ((PILL_START_X + PILL_PAD_X + gpu.text_width + PILL_PAD_X + PILL_GAP_M)
                                * dpr) as i32;

                        let mut found = None;
                        for (i, t) in state.targets.iter().enumerate() {
                            let (dx, dy) = effective_offset(state, i);
                            let bx = t.screen_x + t.width - hud_w + dx;
                            let by = t.screen_y - hud_h - hud_gap + dy;
                            if screen_pt.x < bx
                                || screen_pt.x >= bx + hud_w
                                || screen_pt.y < by
                                || screen_pt.y >= by + hud_h
                            {
                                continue;
                            }
                            let lx = screen_pt.x - bx;
                            let zone = if lx < pill_x {
                                HitZone::Drag
                            } else if lx >= x_off {
                                HitZone::XPill
                            } else {
                                HitZone::TextPill
                            };
                            found = Some((i, zone));
                            break;
                        }
                        found
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((idx, zone)) = hit {
                let _ = SetCapture(hwnd);
                let mut g = canvas().lock().unwrap();
                if let Some(ref mut state) = *g {
                    state.potential_drag = true;
                    state.potential_zone = zone;
                    state.dragging = false;
                    state.drag_idx = idx;
                    state.drag_start_x = screen_pt.x;
                    state.drag_start_y = screen_pt.y;
                    state.live_dx = 0;
                    state.live_dy = 0;
                }
            }
            return LRESULT(0);
        }

        WM_MOUSEMOVE => {
            let cx = (lparam.0 & 0xFFFF) as i16 as i32;
            let cy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut screen_pt = POINT { x: cx, y: cy };
            let _ = ClientToScreen(hwnd, &mut screen_pt);

            let mut should_render = false;

            {
                let mut g = canvas().lock().unwrap();
                if let Some(ref mut state) = *g {
                    if state.potential_drag {
                        let dx = screen_pt.x - state.drag_start_x;
                        let dy = screen_pt.y - state.drag_start_y;
                        if dx.abs() > 4 || dy.abs() > 4 {
                            state.dragging = true;
                            state.potential_drag = false;
                        }
                    }

                    if state.dragging {
                        state.live_dx = screen_pt.x - state.drag_start_x;
                        state.live_dy = screen_pt.y - state.drag_start_y;
                        should_render = true;
                    }
                }
            }

            if should_render {
                render_frame();
            }
            return LRESULT(0);
        }

        WM_LBUTTONUP => {
            let mut was_dragging = false;
            let mut was_potential = false;
            let mut initial_zone = HitZone::None;
            let mut idx = 0;

            {
                let g = canvas().lock().unwrap();
                if let Some(ref state) = *g {
                    was_dragging = state.dragging;
                    was_potential = state.potential_drag;
                    initial_zone = state.potential_zone;
                    idx = state.drag_idx;
                }
            }

            if was_dragging || was_potential {
                let _ = ReleaseCapture();
            }

            if was_dragging {
                let (eid, tid, url, new_dx, new_dy) = {
                    let mut g = canvas().lock().unwrap();
                    if let Some(ref mut state) = *g {
                        let (ldx, ldy) = (state.live_dx, state.live_dy);
                        state.dragging = false;
                        state.potential_drag = false;
                        state.potential_zone = HitZone::None;
                        if let Some(t) = state.targets.get_mut(idx) {
                            t.drag_offset_x += ldx;
                            t.drag_offset_y += ldy;
                            let r = (
                                t.element_id.clone(),
                                state.tab_id,
                                t.media_url.clone(),
                                t.drag_offset_x,
                                t.drag_offset_y,
                            );
                            state.live_dx = 0;
                            state.live_dy = 0;
                            r
                        } else {
                            (String::new(), 0, String::new(), 0, 0)
                        }
                    } else {
                        (String::new(), 0, String::new(), 0, 0)
                    }
                };
                if !eid.is_empty() {
                    write_response(&serde_json::json!({
                        "type": "OVERLAY_DRAG_MOVED",
                        "tabId": tid,
                        "elementId": eid,
                        "mediaUrl": url,
                        "dx": new_dx,
                        "dy": new_dy,
                    }));
                }
                render_frame();
            } else if was_potential {
                let mut menu_info = None;
                let mut dismiss_info = None;

                {
                    let mut g = canvas().lock().unwrap();
                    if let Some(ref mut state) = *g {
                        state.dragging = false;
                        state.potential_drag = false;
                        state.potential_zone = HitZone::None;
                        state.live_dx = 0;
                        state.live_dy = 0;

                        if let Some(t) = state.targets.get(idx) {
                            match initial_zone {
                                HitZone::TextPill => {
                                    if let Some(gpu) = &state.gpu {
                                        let dpr = state.dpr as f32;
                                        let hud_w = (gpu.hud_width * dpr) as i32;
                                        let hud_h = (HUD_H * dpr) as i32;
                                        let hud_gap = (HUD_GAP * dpr) as i32;
                                        let bx = t.screen_x + t.width - hud_w + t.drag_offset_x;
                                        let by = t.screen_y - hud_h - hud_gap + t.drag_offset_y;
                                        menu_info = Some((
                                            t.element_id.clone(),
                                            state.tab_id,
                                            t.media_url.clone(),
                                            bx,
                                            by + hud_h,
                                        ));
                                    }
                                }
                                HitZone::XPill => {
                                    dismiss_info = Some((
                                        t.element_id.clone(),
                                        state.tab_id,
                                        t.media_url.clone(),
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                }

                if let Some((eid, tid, url, menu_x, menu_y)) = menu_info {
                    show_quality_menu(
                        hwnd,
                        POINT {
                            x: menu_x,
                            y: menu_y,
                        },
                        &eid,
                        tid,
                        &url,
                    );
                } else if let Some((eid, tid, url)) = dismiss_info {
                    write_response(&serde_json::json!({
                        "type": "OVERLAY_DISMISS",
                        "tabId": tid,
                        "elementId": eid,
                        "mediaUrl": url,
                    }));
                }
            }

            return LRESULT(0);
        }

        WM_DESTROY => return LRESULT(0),
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[allow(dead_code)]
pub fn hit_zone(state: &CanvasState, sx: i32, sy: i32) -> HitZone {
    if let Some(ref gpu) = state.gpu {
        let dpr = state.dpr as f32;
        let hud_w = (gpu.hud_width * dpr) as i32;
        let hud_h = (HUD_H * dpr) as i32;
        let pill_x = (PILL_START_X * dpr) as i32;
        let x_off =
            ((PILL_START_X + PILL_PAD_X + gpu.text_width + PILL_PAD_X + PILL_GAP_M) * dpr) as i32;

        for i in 0..state.targets.len() {
            let (bx, by) = match hud_screen_pos(state, i) {
                Some(p) => p,
                None => continue,
            };
            if sx < bx || sx >= bx + hud_w || sy < by || sy >= by + hud_h {
                continue;
            }
            let lx = sx - bx;
            return if lx < pill_x {
                HitZone::Drag
            } else if lx >= x_off {
                HitZone::XPill
            } else {
                HitZone::TextPill
            };
        }
    }
    HitZone::None
}

pub fn effective_offset(state: &CanvasState, idx: usize) -> (i32, i32) {
    let base = state
        .targets
        .get(idx)
        .map(|t| (t.drag_offset_x, t.drag_offset_y))
        .unwrap_or((0, 0));
    if state.dragging && state.drag_idx == idx {
        (base.0 + state.live_dx, base.1 + state.live_dy)
    } else {
        base
    }
}

pub fn hud_screen_pos(state: &CanvasState, idx: usize) -> Option<(i32, i32)> {
    let gpu = state.gpu.as_ref()?;
    let dpr = state.dpr as f32;
    let hud_w = (gpu.hud_width * dpr) as i32;
    let hud_h = (HUD_H * dpr) as i32;
    let hud_gap = (HUD_GAP * dpr) as i32;
    let t = state.targets.get(idx)?;
    let (dx, dy) = effective_offset(state, idx);

    let bx = t.screen_x + t.width - hud_w + dx;
    let ideal_by = t.screen_y - hud_h - hud_gap + dy;

    if ideal_by < state.viewport_screen_y {
        return None;
    }

    Some((bx, ideal_by))
}

pub unsafe fn update_window_region(hwnd: HWND, state: &CanvasState) {
    let gpu = match state.gpu.as_ref() {
        Some(g) => g,
        None => {
            let empty = CreateRectRgn(0, 0, 0, 0);
            let _ = SetWindowRgn(hwnd, empty, TRUE);
            return;
        }
    };

    if state.targets.is_empty() {
        let empty = CreateRectRgn(0, 0, 0, 0);
        let _ = SetWindowRgn(hwnd, empty, TRUE);
        return;
    }

    if state.dragging {
        let full = CreateRectRgn(0, 0, state.viewport_width, state.viewport_height);
        let _ = SetWindowRgn(hwnd, full, TRUE);
        return;
    }

    let dpr = state.dpr as f32;
    let hud_w = (gpu.hud_width * dpr) as i32;
    let hud_h = (HUD_H * dpr) as i32;
    let vp_x = state.viewport_screen_x;
    let vp_y = state.viewport_screen_y;

    let combined = CreateRectRgn(0, 0, 0, 0);

    for i in 0..state.targets.len() {
        let (bx, by) = match hud_screen_pos(state, i) {
            Some(p) => p,
            None => continue,
        };

        let rx = bx - vp_x;
        let ry = by - vp_y;

        let btn = CreateRectRgn(rx, ry, rx + hud_w, ry + hud_h);
        CombineRgn(combined, combined, btn, RGN_OR);
        let _ = DeleteObject(btn);
    }

    let _ = SetWindowRgn(hwnd, combined, TRUE);
}
