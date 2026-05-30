// crates/host/src/overlay/render.rs

use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D_POINT_2F, D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;

use crate::overlay::log_debug;
use crate::overlay::types::*;
use crate::overlay::wndproc::{effective_offset, hud_screen_pos, update_window_region};

pub unsafe fn do_render(state: &mut CanvasState) {
    let gpu = match state.gpu.as_ref() {
        Some(g) => g,
        None => return,
    };
    let ctx = &gpu.d2d_ctx;

    ctx.BeginDraw();
    ctx.Clear(Some(&D2D1_COLOR_F {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    }));

    let dpr = state.dpr as f32;
    let vx = state.viewport_screen_x as f32;
    let vy = state.viewport_screen_y as f32;

    for i in 0..state.targets.len() {
        let (bx_phys, by_phys) = match hud_screen_pos(state, i) {
            Some((bx, by)) => (bx as f32, by as f32),
            None => continue,
        };
        let target = &state.targets[i];
        let (dx, dy) = effective_offset(state, i);
        draw_hud(
            ctx, gpu, state, target, dpr, dx, dy, bx_phys, by_phys, vx, vy,
        );
    }

    if let Err(e) = ctx.EndDraw(None, None) {
        log_debug(&format!("[tur] overlay: EndDraw failed: {e:?}"));
        return;
    }
    let hr = gpu.swapchain.Present(1, DXGI_PRESENT(0));
    if hr.is_err() {
        log_debug(&format!("[tur] overlay: Present failed: {:?}", hr));
    }

    let hwnd = HWND(state.hwnd as *mut c_void);
    update_window_region(hwnd, state);
}

unsafe fn draw_hud(
    ctx: &ID2D1DeviceContext,
    gpu: &GpuState,
    state: &CanvasState,
    _target: &TargetInfo,
    dpr: f32,
    _dx: i32,
    _dy: i32,
    bx_phys: f32,
    by_phys: f32,
    vx: f32,
    vy: f32,
) {
    let hud_w = gpu.hud_width;
    let hud_h = HUD_H;

    let bx = (bx_phys - vx) / dpr;
    let by = (by_phys - vy) / dpr;

    let br = &gpu.brush;

    // ── 1. Container ──────────────────────────────────────────────────────────
    let container = D2D_RECT_F {
        left: bx,
        top: by,
        right: bx + hud_w,
        bottom: by + hud_h,
    };
    let (bg_r, bg_g, bg_b, bg_a) = if state.is_dark {
        (0.07_f32, 0.07, 0.07, 0.80) // #121212 80%
    } else {
        (0.94, 0.94, 0.94, 0.88) // #F0F0F0 88%
    };
    br.SetColor(&D2D1_COLOR_F {
        r: bg_r,
        g: bg_g,
        b: bg_b,
        a: bg_a,
    });
    ctx.FillRectangle(&container, br);

    // ── 2. Logo ───────────────────────────────────────────────────────────────
    let logo_l = bx + LOGO_L;
    let logo_t = by + (hud_h - LOGO_SZ) / 2.0;
    let logo_rect = D2D_RECT_F {
        left: logo_l,
        top: logo_t,
        right: logo_l + LOGO_SZ,
        bottom: logo_t + LOGO_SZ,
    };
    if let Some(ref bmp) = gpu.logo_bmp {
        ctx.DrawBitmap(
            bmp,
            Some(&logo_rect),
            1.0,
            D2D1_INTERPOLATION_MODE_LINEAR,
            None,
            None,
        );
    } else {
        let badge_r = D2D1_ROUNDED_RECT {
            rect: logo_rect,
            radiusX: LOGO_SZ / 4.0,
            radiusY: LOGO_SZ / 4.0,
        };
        br.SetColor(&D2D1_COLOR_F {
            r: 0.10,
            g: 0.45,
            b: 0.91,
            a: 1.0,
        });
        ctx.FillRoundedRectangle(&badge_r, br);
    }

    // ── 3. Text pill ──────────────────────────────────────────────────────────
    let text_pill_w = PILL_PAD_X + gpu.text_width + PILL_PAD_X;
    let pill_l = bx + PILL_START_X;
    let pill_t = by + 3.0;
    let pill_b = by + hud_h - 3.0;
    let text_pill = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: pill_l,
            top: pill_t,
            right: pill_l + text_pill_w,
            bottom: pill_b,
        },
        radiusX: PILL_R,
        radiusY: PILL_R,
    };
    let (pill_r, pill_g, pill_b_c, pill_a) = if state.is_dark {
        (1.0_f32, 1.0, 1.0, 0.12)
    } else {
        (0.0, 0.0, 0.0, 0.10)
    };
    br.SetColor(&D2D1_COLOR_F {
        r: pill_r,
        g: pill_g,
        b: pill_b_c,
        a: pill_a,
    });
    ctx.FillRoundedRectangle(&text_pill, br);

    let text_clr = if state.is_dark {
        D2D1_COLOR_F {
            r: 0.91,
            g: 0.92,
            b: 0.93,
            a: 1.0,
        }
    } else {
        D2D1_COLOR_F {
            r: 0.07,
            g: 0.07,
            b: 0.07,
            a: 1.0,
        }
    };
    br.SetColor(&text_clr);
    ctx.DrawTextLayout(
        D2D_POINT_2F {
            x: pill_l + PILL_PAD_X,
            y: by,
        },
        &gpu.text_layout,
        br,
        D2D1_DRAW_TEXT_OPTIONS_NONE,
    );

    // ── 4. X pill ─────────────────────────────────────────────────────────────
    let x_pill_l = pill_l + text_pill_w + PILL_GAP_M;
    let x_pill = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: x_pill_l,
            top: pill_t,
            right: x_pill_l + X_W,
            bottom: pill_b,
        },
        radiusX: PILL_R,
        radiusY: PILL_R,
    };
    br.SetColor(&D2D1_COLOR_F {
        r: pill_r,
        g: pill_g,
        b: pill_b_c,
        a: pill_a,
    });
    ctx.FillRoundedRectangle(&x_pill, br);

    br.SetColor(&text_clr);
    ctx.DrawTextLayout(
        D2D_POINT_2F {
            x: x_pill_l,
            y: by,
        },
        &gpu.x_layout,
        br,
        D2D1_DRAW_TEXT_OPTIONS_NONE,
    );
}
