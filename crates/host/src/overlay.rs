// overlay.rs — Direct2D + DirectComposition hardware overlay
//
// Architecture: WS_EX_NOREDIRECTIONBITMAP window, no GDI redirection surface.
// D3D11 device → DXGI flip-model swapchain (BGRA8, PREMULTIPLIED alpha) →
// D2D1 device context renders into the swapchain back-buffer →
// IDCompositionDevice wires the swapchain to the HWND visual tree →
// DWM composes at 144 Hz with zero flicker, zero magenta key-colour artefacts.
//
// All GDI colour-key (magenta KEY_COLOR) rendering is completely removed.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};

use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{ClientToScreen, PAINTSTRUCT, BeginPaint, EndPaint,
    CreateRectRgn, CombineRgn, DeleteObject, SetWindowRgn, RGN_OR};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetCapture, ReleaseCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

// ── logo bytes embedded at compile time ─────────────────────────────────────
const LOGO_PNG: &[u8] = include_bytes!("../../../extension/icons/32x32.png");

// ── HUD layout constants (logical pixels / DIPs) ─────────────────────────────
const HUD_H:       f32 = 24.0; // button height
const HUD_GAP:     f32 = 4.0;  // gap between button bottom and video top
const LOGO_L:      f32 = 5.0;  // left margin before logo
const LOGO_SZ:     f32 = 14.0; // logo square size
const PILL_GAP_L:  f32 = 4.0;  // gap logo → text pill
const PILL_PAD_X:  f32 = 8.0;  // text pill horizontal inner padding
const PILL_GAP_M:  f32 = 3.0;  // gap text pill → X pill
const X_W:         f32 = 22.0; // X pill width
const R_PAD:       f32 = 5.0;  // right margin after X pill
const PILL_R:      f32 = 4.0;  // pill corner radius
const FONT_SIZE:   f32 = 10.0; // font size in pt

// Logo drag zone: anything left of PILL_START_X is the drag handle
const PILL_START_X: f32 = LOGO_L + LOGO_SZ + PILL_GAP_L; // = 23.0 DIPs

// Quality popup menu command IDs
const MENU_DL_1080P: usize = 1001;
const MENU_DL_720P:  usize = 1002;
const MENU_DL_480P:  usize = 1003;
const MENU_COPY_URL: usize = 1004;

// WS_EX_NOREDIRECTIONBITMAP — unbinds window from GDI redirection bitmap.
// Window content comes entirely from the DComp visual tree / DXGI swapchain.
// Value: 0x00200000 (available since Windows 8, absent from some windows-rs versions)
const WS_EX_NOREDIRECTIONBITMAP: u32 = 0x00200000;

// ── HitZone enum ─────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq)]
enum HitZone { None, Drag, TextPill, XPill }

// ── GPU resource bundle ───────────────────────────────────────────────────────
#[allow(dead_code)] // COM fields kept alive intentionally — dropping them releases GPU resources
struct GpuState {
    d3d:              ID3D11Device,
    d2d_ctx:          ID2D1DeviceContext,
    dwrite:           IDWriteFactory,
    swapchain:        IDXGISwapChain1,
    target_bmp:       Option<ID2D1Bitmap1>, // Option wrapper for safe drops
    dcomp:            IDCompositionDevice,
    _dcomp_tgt:       IDCompositionTarget,
    _dcomp_vis:       IDCompositionVisual,
    brush:            ID2D1SolidColorBrush,
    text_fmt:         IDWriteTextFormat,
    x_fmt:            IDWriteTextFormat,     // Cached center-aligned dismiss text format
    _font_collection: Option<IDWriteFontCollection>, // Keeps custom font loaded in memory
    logo_bmp:         Option<ID2D1Bitmap1>,
    text_width:       f32,
    hud_width:        f32,
    sc_size:          (u32, u32),
}
// COM interface pointers are reference-counted and safe to move across threads
// as long as they are used only on the thread that created them (our message loop).
unsafe impl Send for GpuState {}
unsafe impl Sync for GpuState {}

// ── per-canvas state ──────────────────────────────────────────────────────────
pub struct CanvasState {
    hwnd:               isize,
    tab_id:             i32,
    viewport_screen_x:  i32,
    viewport_screen_y:  i32,
    viewport_width:     i32,
    viewport_height:    i32,
    dpr:                f64,
    targets:            Vec<TargetInfo>,
    is_dark:            bool,
    gpu:                Option<GpuState>,
    // ── drag state ──────────────────────────────────────────────────────────
    potential_drag:     bool,
    potential_zone:     HitZone,
    dragging:           bool,
    drag_idx:           usize,     // index into targets[]
    drag_start_x:       i32,       // screen X where drag started
    drag_start_y:       i32,       // screen Y where drag started
    live_dx:            i32,       // uncommitted delta while dragging
    live_dy:            i32,
}

// ── public types ──────────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct TargetInfo {
    pub element_id:    String,
    pub screen_x:      i32,
    pub screen_y:      i32,
    pub width:         i32,
    pub _height:       i32,
    pub media_url:     String,
    pub drag_offset_x: i32,  // persistent offset from extension storage
    pub drag_offset_y: i32,
}

#[derive(Debug)]
pub struct CanvasUpdate {
    pub tab_id:            i32,
    pub viewport_screen_x: i32,
    pub viewport_screen_y: i32,
    pub viewport_width:    i32,
    pub viewport_height:   i32,
    pub device_pixel_ratio:f64,
    pub targets:           Vec<TargetInfo>,
    pub owner:             isize,
    pub is_dark:           bool,
}

// ── global canvas singleton ───────────────────────────────────────────────────
fn canvas() -> &'static Mutex<Option<CanvasState>> {
    static C: OnceLock<Mutex<Option<CanvasState>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

// ── public API ────────────────────────────────────────────────────────────────

pub fn init() {
    unsafe { register_class() };
}

pub fn update(u: CanvasUpdate) {
    unsafe { do_update(u) };
}

pub fn hide() {
    unsafe { do_hide() };
}

pub fn destroy() {
    unsafe { do_destroy() };
}

/// Re-render the HUD into the D2D context and Present.
/// Safe to call from the message loop, WM_PAINT, or an EVENT_OBJECT_LOCATIONCHANGE hook —
/// all on the same main thread as the message pump.
pub fn render_frame() {
    let mut g = canvas().lock().unwrap();
    if let Some(ref mut state) = *g {
        unsafe { do_render(state); }
    }
}

// ── window class registration ─────────────────────────────────────────────────
static CLASS_ATOM: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

unsafe fn register_class() {
    let instance = HINSTANCE(
        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map(|m| m.0)
            .unwrap_or_default(),
    );
    let wc = WNDCLASSW {
        style:         CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc:   Some(canvas_wndproc),
        hInstance:     instance,
        lpszClassName: w!("TurOverlayCanvas"),
        ..Default::default()
    };
    let atom = RegisterClassW(&wc);
    CLASS_ATOM.store(atom, std::sync::atomic::Ordering::SeqCst);
}

fn log_debug(msg: &str) {
    use std::fs::OpenOptions;
    use std::io::Write;
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"C:\Users\Shah\.gemini\antigravity-ide\brain\f3fdf00f-ff53-4d50-8779-b8b9f6116f8b\scratch\overlay_debug.log")
    {
        let _ = writeln!(file, "{}", msg);
    }
}

// ── update (create / resize / repaint) ───────────────────────────────────────
unsafe fn do_update(u: CanvasUpdate) {
    log_debug(&format!(
        "do_update: tab_id={} targets_len={} vp_screen_x={}, vp_screen_y={}, vp_width={}, vp_height={}, dpr={}",
        u.tab_id, u.targets.len(), u.viewport_screen_x, u.viewport_screen_y, u.viewport_width, u.viewport_height, u.device_pixel_ratio
    ));
    for (i, t) in u.targets.iter().enumerate() {
        log_debug(&format!(
            "  target[{}]: id={} screen_x={}, screen_y={}, width={}, dx={}, dy={}",
            i, t.element_id, t.screen_x, t.screen_y, t.width, t.drag_offset_x, t.drag_offset_y
        ));
    }
    let mut different_tab = false;
    let mut old_hwnd: Option<HWND> = None;
    let mut existing_hwnd: Option<HWND> = None;

    {
        let mut g = canvas().lock().unwrap();
        if let Some(ref mut state) = *g {
            if state.tab_id != u.tab_id {
                different_tab = true;
                if let Some(ref gpu) = state.gpu {
                    gpu.d2d_ctx.SetTarget(None);
                }
                state.gpu = None;
                if state.hwnd != 0 {
                    old_hwnd = Some(HWND(state.hwnd as *mut c_void));
                }
            } else if state.hwnd != 0 {
                existing_hwnd = Some(HWND(state.hwnd as *mut c_void));
            }
        }
    }

    if different_tab {
        if let Some(h) = old_hwnd {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }

    let instance = HINSTANCE(
        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map(|m| m.0)
            .unwrap_or_default(),
    );

    let owner = if u.owner != 0 { HWND(u.owner as *mut c_void) } else { HWND(null_mut()) };

    let new_hwnd = if let Some(hwnd) = existing_hwnd {
        // Reposition / resize existing window.
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            u.viewport_screen_x, u.viewport_screen_y,
            u.viewport_width, u.viewport_height,
            SWP_NOACTIVATE,
        );
        hwnd
    } else {
        // Create the overlay window.
        // WS_EX_NOREDIRECTIONBITMAP: detaches from GDI redirection bitmap so DComp
        //   swapchain provides all visual content (no magenta key-colour artefacts).
        // WS_EX_TOPMOST / WS_EX_TOOLWINDOW: keep above browser, no taskbar entry.
        // WS_EX_NOACTIVATE: mouse clicks on the overlay do NOT steal focus from browser.
        // NOTE: Do NOT add WS_EX_TRANSPARENT — that flag causes Windows to skip
        //   this window entirely during hit-testing, meaning WM_NCHITTEST is never
        //   dispatched. Passthrough is handled by returning HTTRANSPARENT from
        //   WM_NCHITTEST for all non-HUD regions (already implemented below).
        // NOTE: Do NOT add WS_EX_LAYERED — redundant with the DComp/DXGI alpha path
        //   and can interfere with DWM hit-testing on some driver stacks.
        let ex_style = WINDOW_EX_STYLE(
            WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOREDIRECTIONBITMAP
            | WS_EX_NOACTIVATE.0,
        );
        CreateWindowExW(
            ex_style,
            w!("TurOverlayCanvas"),
            w!(""),
            WS_POPUP,
            u.viewport_screen_x,
            u.viewport_screen_y,
            u.viewport_width,
            u.viewport_height,
            owner,
            HMENU(null_mut()),
            instance,
            None,
        ).unwrap_or(HWND(null_mut()))
    };

    if new_hwnd.is_invalid() || new_hwnd == HWND(null_mut()) {
        eprintln!("[tur] overlay: CreateWindowExW failed");
        return;
    }

    // ── Update or create canvas state ────────────────────────────────────────
    let phys_w = u.viewport_width.max(1) as u32;
    let phys_h = u.viewport_height.max(1) as u32;
    let dpi = (u.device_pixel_ratio * 96.0) as f32;

    let mut need_init = false;

    {
        let mut g = canvas().lock().unwrap();
        if g.is_none() {
            *g = Some(CanvasState {
                hwnd: new_hwnd.0 as isize,
                tab_id: u.tab_id,
                viewport_screen_x: u.viewport_screen_x,
                viewport_screen_y: u.viewport_screen_y,
                viewport_width: u.viewport_width,
                viewport_height: u.viewport_height,
                dpr: u.device_pixel_ratio,
                targets: u.targets.clone(),
                is_dark: u.is_dark,
                gpu: None,
                potential_drag: false,
                potential_zone: HitZone::None,
                dragging: false,
                drag_idx: 0,
                drag_start_x: 0,
                drag_start_y: 0,
                live_dx: 0,
                live_dy: 0,
            });
            need_init = true;
        } else if let Some(ref mut state) = *g {
            state.hwnd = new_hwnd.0 as isize;
            state.tab_id = u.tab_id;
            state.viewport_screen_x = u.viewport_screen_x;
            state.viewport_screen_y = u.viewport_screen_y;
            state.viewport_width = u.viewport_width;
            state.viewport_height = u.viewport_height;
            state.dpr = u.device_pixel_ratio;
            state.targets = u.targets.clone();
            state.is_dark = u.is_dark;
            if state.gpu.is_none() {
                need_init = true;
            }
        }
    }

    if need_init {
        match init_gpu(new_hwnd, phys_w, phys_h, dpi) {
            Ok(gpu) => {
                let mut g = canvas().lock().unwrap();
                if let Some(ref mut state) = *g {
                    state.gpu = Some(gpu);
                }
            }
            Err(e) => {
                eprintln!("[tur] overlay: GPU init failed: {e:?}");
                return;
            }
        }
    } else {
        // Resize swapchain if dimensions changed.
        let mut g = canvas().lock().unwrap();
        if let Some(ref mut state) = *g {
            if let Some(ref mut gpu) = state.gpu {
                if gpu.sc_size != (phys_w, phys_h) {
                    if let Err(e) = resize_swapchain(gpu, phys_w, phys_h, dpi) {
                        eprintln!("[tur] overlay: swapchain resize failed: {e:?}");
                    }
                }
            }
        }
    }

    let _ = ShowWindow(new_hwnd, SW_SHOWNOACTIVATE);

    // Render immediately — don't wait for WM_PAINT.
    let mut g = canvas().lock().unwrap();
    if let Some(ref mut state) = *g {
        do_render(state);
    }
}

// ── GPU device + swapchain + DComp initialisation ────────────────────────────
unsafe fn init_gpu(
    hwnd: HWND,
    w: u32, h: u32,
    dpi: f32,
) -> windows::core::Result<GpuState> {
    // ── D3D11 device ─────────────────────────────────────────────────────────
    let mut d3d: Option<ID3D11Device> = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        None,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, // required for D2D interop
        None,
        D3D11_SDK_VERSION,
        Some(&mut d3d),
        None,
        None,
    )?;
    let d3d = d3d.unwrap();

    // ── D2D1 factory + device + context ──────────────────────────────────────
    let d2d_factory: ID2D1Factory1 =
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None::<*const D2D1_FACTORY_OPTIONS>)?;
    let dxgi_dev: IDXGIDevice = d3d.cast()?;
    let d2d_dev = d2d_factory.CreateDevice(&dxgi_dev)?;
    let d2d_ctx: ID2D1DeviceContext =
        d2d_dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
    d2d_ctx.SetDpi(dpi, dpi);
    d2d_ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    d2d_ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);

    // ── DXGI swapchain ───────────────────────────────────────────────────────
    let dxgi_adapter: IDXGIAdapter = dxgi_dev.GetAdapter()?;
    let dxgi_factory: IDXGIFactory2 = dxgi_adapter.GetParent()?;
    let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
        Width:       w,
        Height:      h,
        Format:      DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo:      FALSE,
        SampleDesc:  DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling:     DXGI_SCALING_STRETCH,
        SwapEffect:  DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode:   DXGI_ALPHA_MODE_PREMULTIPLIED, // ← hardware alpha, no key colour
        Flags:       0,
    };
    let swapchain: IDXGISwapChain1 =
        dxgi_factory.CreateSwapChainForComposition(&d3d, &sc_desc, None)?;

    // ── Bind D2D context to swapchain back-buffer ─────────────────────────────
    let target_bmp = create_d2d_target(&d2d_ctx, &swapchain, dpi)?;
    d2d_ctx.SetTarget(&target_bmp);

    // ── DirectComposition visual tree ─────────────────────────────────────────
    let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_dev)?;
    let dcomp_tgt = dcomp.CreateTargetForHwnd(hwnd, true)?;
    let dcomp_vis = dcomp.CreateVisual()?;
    dcomp_vis.SetContent(&swapchain)?;
    dcomp_tgt.SetRoot(&dcomp_vis)?;
    dcomp.Commit()?;

    // ── DirectWrite factory + text formats + Custom Fonts ────────────────────
    let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
    
    // Find the executable directory to locate font files
    let mut path_buf = [0u16; 1024];
    let len = windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut path_buf) as usize;
    let mut font_col: Option<IDWriteFontCollection> = None;
    if len > 0 {
        let exe_path = String::from_utf16_lossy(&path_buf[..len]);
        if let Some(exe_dir) = std::path::Path::new(&exe_path).parent() {
            let sans_path = exe_dir.join("InstrumentSans.ttf");
            let murmure_path = exe_dir.join("LeMurmure.otf");
            
            let factory3_res: Result<IDWriteFactory3, _> = dwrite.cast();
            if let Ok(factory3) = factory3_res {
                let mut loaded_files = Vec::new();
                if sans_path.exists() {
                    let file_hstr = windows::core::HSTRING::from(sans_path.as_os_str());
                    if let Ok(file) = dwrite.CreateFontFileReference(&file_hstr, None) {
                        loaded_files.push(file);
                    }
                } else {
                    eprintln!("[tur] InstrumentSans.ttf not found at {:?}", sans_path);
                }
                if murmure_path.exists() {
                    let file_hstr = windows::core::HSTRING::from(murmure_path.as_os_str());
                    if let Ok(file) = dwrite.CreateFontFileReference(&file_hstr, None) {
                        loaded_files.push(file);
                    }
                } else {
                    eprintln!("[tur] LeMurmure.otf not found at {:?}", murmure_path);
                }

                if !loaded_files.is_empty() {
                    if let Ok(builder) = factory3.CreateFontSetBuilder() {
                        for file in &loaded_files {
                            if let Ok(face_ref) = factory3.CreateFontFaceReference(file, 0, DWRITE_FONT_SIMULATIONS_NONE) {
                                let _ = builder.AddFontFaceReference(&face_ref, &[]);
                            }
                        }
                        if let Ok(font_set) = builder.CreateFontSet() {
                            if let Ok(col1) = factory3.CreateFontCollectionFromFontSet(&font_set) {
                                if let Ok(col) = col1.cast::<IDWriteFontCollection>() {
                                    font_col = Some(col);
                                    eprintln!("[tur] Loaded custom fonts successfully!");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let font_collection_ref = font_col.as_ref();
    let text_fmt: IDWriteTextFormat = dwrite.CreateTextFormat(
        w!("Instrument Sans"), // Render in Instrument Sans if loaded, fallback to Segoe UI
        font_collection_ref,
        DWRITE_FONT_WEIGHT_NORMAL,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        FONT_SIZE,
        w!("en-us"),
    )?;
    text_fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
    text_fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

    // X glyph text format (Segoe UI, center aligned, 13 pt)
    let x_fmt: IDWriteTextFormat = dwrite.CreateTextFormat(
        w!("Segoe UI"),
        None,
        DWRITE_FONT_WEIGHT_NORMAL,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        13.0,
        w!("en-us"),
    )?;
    x_fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
    x_fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

    // ── Measure "Download with tur" text width ────────────────────────────────
    let label_utf16: Vec<u16> = "Download with tur".encode_utf16().collect();
    let layout: IDWriteTextLayout = dwrite.CreateTextLayout(
        &label_utf16, &text_fmt, 2000.0, HUD_H,
    )?;
    let mut metrics = DWRITE_TEXT_METRICS::default();
    layout.GetMetrics(&mut metrics)?;
    let text_width = metrics.width;
    let text_pill_width = PILL_PAD_X + text_width + PILL_PAD_X;
    let hud_width = PILL_START_X + text_pill_width + PILL_GAP_M + X_W + R_PAD;

    // ── D2D solid colour brush ────────────────────────────────────────────────
    let rt: ID2D1RenderTarget = d2d_ctx.cast()?;
    let brush: ID2D1SolidColorBrush = rt.CreateSolidColorBrush(
        &D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
        None,
    )?;
    drop(rt);

    // ── WIC PNG logo decode ───────────────────────────────────────────────────
    let logo_bmp = load_logo(&d2d_ctx, dpi).ok();

    eprintln!("[tur] overlay: GPU init OK text_w={text_width:.1} hud_w={hud_width:.1} dpi={dpi}");

    Ok(GpuState {
        d3d, d2d_ctx, dwrite, swapchain, target_bmp: Some(target_bmp), dcomp,
        _dcomp_tgt: dcomp_tgt, _dcomp_vis: dcomp_vis,
        brush, text_fmt, x_fmt, _font_collection: font_col, logo_bmp, text_width, hud_width,
        sc_size: (w, h),
    })
}

unsafe fn create_d2d_target(
    ctx: &ID2D1DeviceContext,
    sc: &IDXGISwapChain1,
    dpi: f32,
) -> windows::core::Result<ID2D1Bitmap1> {
    let surface: IDXGISurface = sc.GetBuffer(0)?;
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format:    DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: ManuallyDrop::new(None),
    };
    ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))
}

unsafe fn resize_swapchain(
    gpu: &mut GpuState,
    w: u32, h: u32,
    dpi: f32,
) -> windows::core::Result<()> {
    gpu.d2d_ctx.SetTarget(None); // release D2D ref to the back-buffer bitmap
    // Drop old target bitmap before ResizeBuffers (COM refcount must reach 0)
    gpu.target_bmp = None;
    // Now resize
    gpu.swapchain.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
    let new_target = create_d2d_target(&gpu.d2d_ctx, &gpu.swapchain, dpi)?;
    gpu.d2d_ctx.SetTarget(&new_target);
    gpu.target_bmp = Some(new_target);
    gpu.sc_size = (w, h);
    Ok(())
}

// ── WIC logo decode ───────────────────────────────────────────────────────────
unsafe fn load_logo(ctx: &ID2D1DeviceContext, dpi: f32) -> windows::core::Result<ID2D1Bitmap1> {
    // CoInitializeEx has already been called in main() — no init here.
    let factory: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let stream: IWICStream = factory.CreateStream()?;
    stream.InitializeFromMemory(LOGO_PNG)?;
    let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
    let frame: IWICBitmapFrameDecode = decoder.GetFrame(0)?;
    let converter: IWICFormatConverter = factory.CreateFormatConverter()?;
    converter.Initialize(
        &frame,
        &GUID_WICPixelFormat32bppPBGRA, // premultiplied BGRA — matches D2D
        WICBitmapDitherTypeNone,
        None,
        0.0,
        WICBitmapPaletteTypeCustom,
    )?;
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format:    DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
        colorContext: ManuallyDrop::new(None),
    };
    ctx.CreateBitmapFromWicBitmap(&converter, Some(&props))
}

// ── hide / destroy ────────────────────────────────────────────────────────────
unsafe fn do_hide() {
    let g = canvas().lock().unwrap();
    if let Some(ref state) = *g {
        if state.hwnd != 0 {
            let _ = ShowWindow(HWND(state.hwnd as *mut c_void), SW_HIDE);
        }
    }
}

unsafe fn do_destroy() {
    let mut g = canvas().lock().unwrap();
    if let Some(ref mut state) = *g {
        if let Some(ref gpu) = state.gpu {
            gpu.d2d_ctx.SetTarget(None);
        }
        drop(state.gpu.take());
        if state.hwnd != 0 {
            let _ = DestroyWindow(HWND(state.hwnd as *mut c_void));
            state.hwnd = 0;
        }
    }
}

// ── window procedure ──────────────────────────────────────────────────────────
unsafe extern "system" fn canvas_wndproc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => return LRESULT(1),

        WM_PAINT => {
            // DComp composites the swapchain — just validate the dirty rect.
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            // Re-render in case DWM needs a fresh frame (e.g. after minimize restore).
            render_frame();
            return LRESULT(0);
        }

        WM_ERASEBKGND => return LRESULT(1),

        WM_NCHITTEST => {
            // SetWindowRgn already clips the input region to the HUD button area,
            // so this handler is only reached when the cursor is over the button.
            // Just return HTCLIENT so we get WM_LBUTTONDOWN etc.
            return LRESULT(HTCLIENT as isize);
        }

        WM_LBUTTONDOWN => {
            let cx = (lparam.0 & 0xFFFF) as i16 as i32;
            let cy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut screen_pt = POINT { x: cx, y: cy };
            let _ = ClientToScreen(hwnd, &mut screen_pt);

            // Collect hit info without holding the lock during WinAPI calls.
            let hit = {
                let g = canvas().lock().unwrap();
                if let Some(ref state) = *g {
                    if let Some(ref gpu) = state.gpu {
                        let dpr = state.dpr as f32;
                        let hud_w = (gpu.hud_width  * dpr) as i32;
                        let hud_h = (HUD_H         * dpr) as i32;
                        let hud_gap = (HUD_GAP     * dpr) as i32;
                        let pill_x = (PILL_START_X * dpr) as i32;
                        let x_off  = ((PILL_START_X + PILL_PAD_X + gpu.text_width + PILL_PAD_X + PILL_GAP_M) * dpr) as i32;

                        let mut found = None;
                        for (i, t) in state.targets.iter().enumerate() {
                            let (dx, dy) = effective_offset(state, i);
                            let bx = t.screen_x + t.width - hud_w + dx;
                            let by = t.screen_y - hud_h - hud_gap + dy;
                            if screen_pt.x < bx || screen_pt.x >= bx + hud_w || screen_pt.y < by || screen_pt.y >= by + hud_h { continue; }
                            let lx = screen_pt.x - bx;
                            let zone = if lx < pill_x { HitZone::Drag }
                                       else if lx >= x_off { HitZone::XPill }
                                       else { HitZone::TextPill };
                            found = Some((i, zone));
                            break;
                        }
                        found
                    } else { None }
                } else { None }
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
                            let r = (t.element_id.clone(), state.tab_id, t.media_url.clone(),
                                     t.drag_offset_x, t.drag_offset_y);
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
                                        menu_info = Some((t.element_id.clone(), state.tab_id, t.media_url.clone(), bx, by + hud_h));
                                    }
                                }
                                HitZone::XPill => {
                                    dismiss_info = Some((t.element_id.clone(), state.tab_id, t.media_url.clone()));
                                }
                                _ => {}
                            }
                        }
                    }
                }

                if let Some((eid, tid, url, menu_x, menu_y)) = menu_info {
                    show_quality_menu(hwnd, POINT { x: menu_x, y: menu_y }, &eid, tid, &url);
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

/// Returns the hit zone for a screen coordinate over any HUD button.
/// Uses `hud_screen_pos` so the hide-above-viewport rule is consistent.
/// (Kept for potential future use; input routing is handled via SetWindowRgn.)
#[allow(dead_code)]
fn hit_zone(state: &CanvasState, sx: i32, sy: i32) -> HitZone {
    if let Some(ref gpu) = state.gpu {
        let dpr    = state.dpr as f32;
        let hud_w  = (gpu.hud_width  * dpr) as i32;
        let hud_h  = (HUD_H          * dpr) as i32;
        let pill_x = (PILL_START_X   * dpr) as i32;
        let x_off  = ((PILL_START_X + PILL_PAD_X + gpu.text_width + PILL_PAD_X + PILL_GAP_M) * dpr) as i32;

        for i in 0..state.targets.len() {
            let (bx, by) = match hud_screen_pos(state, i) {
                Some(p) => p,
                None    => continue, // button hidden — not hittable
            };
            if sx < bx || sx >= bx + hud_w || sy < by || sy >= by + hud_h { continue; }
            let lx = sx - bx;
            return if lx < pill_x      { HitZone::Drag }
                   else if lx >= x_off { HitZone::XPill }
                   else                { HitZone::TextPill };
        }
    }
    HitZone::None
}

fn effective_offset(state: &CanvasState, idx: usize) -> (i32, i32) {
    let base = state.targets.get(idx)
        .map(|t| (t.drag_offset_x, t.drag_offset_y))
        .unwrap_or((0, 0));
    if state.dragging && state.drag_idx == idx {
        (base.0 + state.live_dx, base.1 + state.live_dy)
    } else {
        base
    }
}

/// Compute the screen-space top-left of the HUD button for a given target.
///
/// Returns `None` when the button would sit above the viewport top edge,
/// matching the debug overlay rule: yellow box only drawn when `ay >= 0`.
/// When `None` the button is neither rendered nor included in the input region.
fn hud_screen_pos(state: &CanvasState, idx: usize) -> Option<(i32, i32)> {
    let gpu = state.gpu.as_ref()?;
    let dpr     = state.dpr as f32;
    let hud_w   = (gpu.hud_width * dpr) as i32;
    let hud_h   = (HUD_H         * dpr) as i32;
    let hud_gap = (HUD_GAP       * dpr) as i32;
    let t       = state.targets.get(idx)?;
    let (dx, dy) = effective_offset(state, idx);

    let bx      = t.screen_x + t.width - hud_w + dx;
    let ideal_by = t.screen_y - hud_h - hud_gap + dy;

    // Hide the button entirely when it would clip above the viewport top.
    // This matches the JS debug overlay: yellow box only drawn when ay >= 0.
    if ideal_by < state.viewport_screen_y {
        return None;
    }

    Some((bx, ideal_by))
}

/// Restrict the overlay window's input region to only the HUD button rects.
///
/// Windows uses the window region to determine which pixels "belong" to a window
/// for the purposes of mouse-hit-testing. Pixels outside the region are treated
/// as if the window is not there — input passes directly to whatever is behind,
/// crossing process boundaries transparently.  This is the only correct cross-
/// process passthrough mechanism; WM_NCHITTEST/HTTRANSPARENT only works within
/// the same thread and is therefore useless for a browser overlay.
///
/// During a drag we expand the region to the full viewport because SetCapture
/// already routes all events to us — the region just prevents the cursor from
/// flipping back to the browser cursor mid-drag.
unsafe fn update_window_region(hwnd: HWND, state: &CanvasState) {
    let gpu = match state.gpu.as_ref() {
        Some(g) => g,
        None => {
            // No GPU yet — make window fully transparent to input.
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

    // During an active drag: full-viewport region so SetCapture keeps working
    // and the cursor stays correct while the button is being moved.
    if state.dragging {
        let full = CreateRectRgn(
            0, 0,
            state.viewport_width,
            state.viewport_height,
        );
        let _ = SetWindowRgn(hwnd, full, TRUE);
        return;
    }

    let dpr   = state.dpr as f32;
    let hud_w = (gpu.hud_width * dpr) as i32;
    let hud_h = (HUD_H         * dpr) as i32;
    let vp_x  = state.viewport_screen_x;
    let vp_y  = state.viewport_screen_y;

    // Start with an empty region and OR-in each visible HUD button rect.
    let combined = CreateRectRgn(0, 0, 0, 0);

    for i in 0..state.targets.len() {
        // hud_screen_pos() returns None when the button is above the viewport.
        let (bx, by) = match hud_screen_pos(state, i) {
            Some(p) => p,
            None    => continue, // button hidden — exclude from input region
        };

        // Convert from screen coords to window-client coords.
        let rx = bx - vp_x;
        let ry = by - vp_y;

        let btn = CreateRectRgn(rx, ry, rx + hud_w, ry + hud_h);
        CombineRgn(combined, combined, btn, RGN_OR);
        // CombineRgn copies data out of btn; we own btn and must free it.
        let _ = DeleteObject(btn);
    }

    // SetWindowRgn takes ownership of the region handle — do NOT delete it.
    let _ = SetWindowRgn(hwnd, combined, TRUE);
}

// ── rendering ─────────────────────────────────────────────────────────────────
unsafe fn do_render(state: &mut CanvasState) {
    let gpu = match state.gpu.as_ref() {
        Some(g) => g,
        None => return,
    };
    let ctx = &gpu.d2d_ctx;
    let _dpr = state.dpr as f32;

    ctx.BeginDraw();
    // Clear to fully transparent — DComp composites this over the desktop.
    ctx.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

    let dpr = state.dpr as f32;
    let vx  = state.viewport_screen_x as f32;
    let vy  = state.viewport_screen_y as f32;

    for i in 0..state.targets.len() {
        // Skip buttons that are above the viewport top (match debug overlay rule).
        let (bx_phys, by_phys) = match hud_screen_pos(state, i) {
            Some((bx, by)) => (bx as f32, by as f32),
            None           => continue,
        };
        let target = &state.targets[i];
        let (dx, dy) = effective_offset(state, i);
        draw_hud(ctx, gpu, state, target, dpr, dx, dy, bx_phys, by_phys, vx, vy);
    }

    if let Err(e) = ctx.EndDraw(None, None) {
        eprintln!("[tur] overlay: EndDraw failed: {e:?}");
        return;
    }
    let hr = gpu.swapchain.Present(1, DXGI_PRESENT(0));
    if hr.is_err() {
        eprintln!("[tur] overlay: Present failed: {:?}", hr);
    }

    // Keep the input region in sync with the rendered HUD position.
    // During a drag we set a full-viewport region (SetCapture handles routing)
    // so the cursor doesn't snap; otherwise restrict to HUD rects only so all
    // other input falls straight through to the browser (cross-process safe).
    let hwnd = HWND(state.hwnd as *mut c_void);
    update_window_region(hwnd, state);
}

/// Draw one HUD button for `target` at its effective screen position.
/// `bx_phys` / `by_phys` are pre-computed physical screen coords from `hud_screen_pos`.
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

    // Convert physical screen coords to DIPs relative to our canvas window origin.
    let bx = (bx_phys - vx) / dpr;
    let by = (by_phys - vy) / dpr;

    let br = &gpu.brush;

    // ── 1. Container: flat rectangle, subtle semi-transparent dark bg ─────────
    let container = D2D_RECT_F { left: bx, top: by, right: bx + hud_w, bottom: by + hud_h };
    let (bg_r, bg_g, bg_b, bg_a) = if state.is_dark {
        (0.07_f32, 0.07, 0.07, 0.80) // #121212 80%
    } else {
        (0.94, 0.94, 0.94, 0.88)     // #F0F0F0 88%
    };
    br.SetColor(&D2D1_COLOR_F { r: bg_r, g: bg_g, b: bg_b, a: bg_a });
    ctx.FillRectangle(&container, br); // flat — NO rounded corners on container

    // ── 2. Logo ───────────────────────────────────────────────────────────────
    let logo_l = bx + LOGO_L;
    let logo_t = by + (hud_h - LOGO_SZ) / 2.0;
    let logo_rect = D2D_RECT_F {
        left:   logo_l,
        top:    logo_t,
        right:  logo_l + LOGO_SZ,
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
        // Fallback: brand-blue "T" badge if logo failed to load
        let badge_r = D2D1_ROUNDED_RECT {
            rect: logo_rect,
            radiusX: LOGO_SZ / 4.0,
            radiusY: LOGO_SZ / 4.0,
        };
        br.SetColor(&D2D1_COLOR_F { r: 0.10, g: 0.45, b: 0.91, a: 1.0 });
        ctx.FillRoundedRectangle(&badge_r, br);
    }

    // ── 3. Text pill: "Download with tur" ─────────────────────────────────────
    let text_pill_w = PILL_PAD_X + gpu.text_width + PILL_PAD_X;
    let pill_l = bx + PILL_START_X;
    let pill_t = by + 3.0;
    let pill_b = by + hud_h - 3.0;
    let text_pill = D2D1_ROUNDED_RECT {
        rect:    D2D_RECT_F { left: pill_l, top: pill_t, right: pill_l + text_pill_w, bottom: pill_b },
        radiusX: PILL_R,
        radiusY: PILL_R,
    };
    let (pill_r, pill_g, pill_b_c, pill_a) = if state.is_dark {
        (1.0_f32, 1.0, 1.0, 0.12)
    } else {
        (0.0, 0.0, 0.0, 0.10)
    };
    br.SetColor(&D2D1_COLOR_F { r: pill_r, g: pill_g, b: pill_b_c, a: pill_a });
    ctx.FillRoundedRectangle(&text_pill, br);

    let text_clr = if state.is_dark {
        D2D1_COLOR_F { r: 0.91, g: 0.92, b: 0.93, a: 1.0 }
    } else {
        D2D1_COLOR_F { r: 0.07, g: 0.07, b: 0.07, a: 1.0 }
    };
    br.SetColor(&text_clr);
    let label_utf16: Vec<u16> = "Download with tur".encode_utf16().collect();
    let text_rect = D2D_RECT_F {
        left:   pill_l + PILL_PAD_X,
        top:    by,
        right:  pill_l + text_pill_w - PILL_PAD_X,
        bottom: by + hud_h,
    };
    ctx.DrawText(
        &label_utf16,
        &gpu.text_fmt,
        &text_rect,
        br,
        D2D1_DRAW_TEXT_OPTIONS_NONE,
        DWRITE_MEASURING_MODE_NATURAL,
    );

    // ── 4. X pill ─────────────────────────────────────────────────────────────
    let x_pill_l = pill_l + text_pill_w + PILL_GAP_M;
    let x_pill = D2D1_ROUNDED_RECT {
        rect:    D2D_RECT_F { left: x_pill_l, top: pill_t, right: x_pill_l + X_W, bottom: pill_b },
        radiusX: PILL_R,
        radiusY: PILL_R,
    };
    br.SetColor(&D2D1_COLOR_F { r: pill_r, g: pill_g, b: pill_b_c, a: pill_a });
    ctx.FillRoundedRectangle(&x_pill, br);

    br.SetColor(&text_clr);
    let x_utf16: Vec<u16> = "×".encode_utf16().collect();
    let x_rect = D2D_RECT_F {
        left:   x_pill_l,
        top:    by,
        right:  x_pill_l + X_W,
        bottom: by + hud_h,
    };
    ctx.DrawText(
        &x_utf16, &gpu.x_fmt, &x_rect, br,
        D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
    );
}

// ── quality popup menu (unchanged logic, anchored to button bottom-left) ───────
unsafe fn show_quality_menu(hwnd: HWND, pt: POINT, element_id: &str, tab_id: i32, media_url: &str) {
    macro_rules! wstr {
        ($s:expr) => {{ let v: Vec<u16> = $s.encode_utf16().chain(Some(0)).collect(); v }};
    }
    let menu = match CreatePopupMenu() { Ok(m) => m, Err(_) => return };
    let s1 = wstr!("Download 1080p (stub)");
    let s2 = wstr!("Download 720p (stub)");
    let s3 = wstr!("Download 480p (stub)");
    let s4 = wstr!("Copy Media URL");
    let _ = AppendMenuW(menu, MF_STRING, MENU_DL_1080P, PCWSTR(s1.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, MENU_DL_720P,  PCWSTR(s2.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, MENU_DL_480P,  PCWSTR(s3.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(null_mut()));
    let _ = AppendMenuW(menu, MF_STRING, MENU_COPY_URL, PCWSTR(s4.as_ptr()));
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN,
        pt.x, pt.y, 0, hwnd, None,
    );
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    let cmd = cmd.0 as usize;
    if cmd == 0 { return; }
    let quality = match cmd {
        MENU_DL_1080P => "1080p",
        MENU_DL_720P  => "720p",
        MENU_DL_480P  => "480p",
        _ => "",
    };
    if cmd == MENU_COPY_URL {
        write_response(&serde_json::json!({
            "type": "OVERLAY_COPY_URL",
            "tabId": tab_id,
            "elementId": element_id,
            "mediaUrl": media_url,
        }));
    } else if !quality.is_empty() {
        write_response(&serde_json::json!({
            "type": "OVERLAY_MENU_SELECTED",
            "tabId": tab_id,
            "elementId": element_id,
            "quality": quality,
            "mediaUrl": media_url,
        }));
    }
}

// ── IPC back to extension ─────────────────────────────────────────────────────
fn write_response(value: &serde_json::Value) {
    use std::io::Write;
    let json = serde_json::to_string(value).unwrap_or_default();
    let len = (json.len() as u32).to_le_bytes();
    let mut out = std::io::stdout();
    let _ = out.write_all(&len);
    let _ = out.write_all(json.as_bytes());
    let _ = out.flush();
}
