// crates/host/src/overlay/types.rs


use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext, ID2D1SolidColorBrush};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFontCollection, IDWriteTextLayout};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain1;

// ── HUD layout constants (logical pixels / DIPs) ─────────────────────────────
pub const HUD_H: f32 = 24.0; // button height
pub const HUD_GAP: f32 = 4.0; // gap between button bottom and video top
pub const LOGO_L: f32 = 5.0; // left margin before logo
pub const LOGO_SZ: f32 = 14.0; // logo square size
pub const PILL_GAP_L: f32 = 4.0; // gap logo → text pill
pub const PILL_PAD_X: f32 = 8.0; // text pill horizontal inner padding
pub const PILL_GAP_M: f32 = 3.0; // gap text pill → X pill
pub const X_W: f32 = 22.0; // X pill width
pub const R_PAD: f32 = 5.0; // right margin after X pill
pub const PILL_R: f32 = 4.0; // pill corner radius
pub const FONT_SIZE: f32 = 10.0; // font size in pt

// Logo drag zone: anything left of PILL_START_X is the drag handle
pub const PILL_START_X: f32 = LOGO_L + LOGO_SZ + PILL_GAP_L; // = 23.0 DIPs

// Quality popup menu command IDs
pub const MENU_DL_1080P: usize = 1001;
pub const MENU_DL_720P: usize = 1002;
pub const MENU_DL_480P: usize = 1003;
pub const MENU_COPY_URL: usize = 1004;



// ── HitZone enum ─────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitZone {
    None,
    Drag,
    TextPill,
    XPill,
}

// ── GPU resource bundle ───────────────────────────────────────────────────────
#[allow(dead_code)]
pub struct GpuState {
    pub d3d: ID3D11Device,
    pub d2d_ctx: ID2D1DeviceContext,
    pub dwrite: windows::Win32::Graphics::DirectWrite::IDWriteFactory,
    pub swapchain: IDXGISwapChain1,
    pub target_bmp: Option<ID2D1Bitmap1>,
    pub dcomp: IDCompositionDevice,
    pub _dcomp_tgt: IDCompositionTarget,
    pub _dcomp_vis: IDCompositionVisual,
    pub brush: ID2D1SolidColorBrush,
    pub text_layout: IDWriteTextLayout,
    pub x_layout: IDWriteTextLayout,
    pub _font_collection: Option<IDWriteFontCollection>,
    pub logo_bmp: Option<ID2D1Bitmap1>,
    pub text_width: f32,
    pub hud_width: f32,
    pub sc_size: (u32, u32),
}

unsafe impl Send for GpuState {}
unsafe impl Sync for GpuState {}

// ── per-canvas state ──────────────────────────────────────────────────────────
pub struct CanvasState {
    pub hwnd: isize,
    pub tab_id: i32,
    pub viewport_screen_x: i32,
    pub viewport_screen_y: i32,
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub dpr: f64,
    pub targets: Vec<TargetInfo>,
    pub is_dark: bool,
    pub gpu: Option<GpuState>,
    // ── drag state ──────────────────────────────────────────────────────────
    pub potential_drag: bool,
    pub potential_zone: HitZone,
    pub dragging: bool,
    pub drag_idx: usize,
    pub drag_start_x: i32,
    pub drag_start_y: i32,
    pub live_dx: i32,
    pub live_dy: i32,
}

pub use crate::types::{TargetInfo, CanvasUpdate};
