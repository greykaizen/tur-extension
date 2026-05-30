// crates/host/src/overlay/gpu.rs

use std::mem::ManuallyDrop;
use windows::core::{w, Interface};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::overlay::log_debug;
use crate::overlay::types::*;
use crate::overlay::wic::load_logo;

pub unsafe fn init_gpu(hwnd: HWND, w: u32, h: u32, dpi: f32) -> windows::core::Result<GpuState> {
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
    let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(
        D2D1_FACTORY_TYPE_SINGLE_THREADED,
        None::<*const D2D1_FACTORY_OPTIONS>,
    )?;
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
        Width: w,
        Height: h,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: FALSE,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED, // ← hardware alpha, no key colour
        Flags: 0,
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
    let len =
        windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut path_buf) as usize;
    let mut font_col: Option<IDWriteFontCollection> = None;
    if len > 0 {
        let exe_path = String::from_utf16_lossy(&path_buf[..len]);
        if let Some(exe_dir) = std::path::Path::new(&exe_path).parent() {
            let sans_path = exe_dir.join("InstrumentSans.ttf");
            let murmure_path = exe_dir.join("LeMurmure.otf");

            let factory3_res: Result<IDWriteFactory3, _> = dwrite.cast();
            if let Ok(factory3) = factory3_res {
                let mut loaded_files = Vec::new();
                log_debug(&format!("[tur] Sans path exists: {}", sans_path.exists()));
                if sans_path.exists() {
                    let file_hstr = windows::core::HSTRING::from(sans_path.as_os_str());
                    match dwrite.CreateFontFileReference(&file_hstr, None) {
                        Ok(file) => {
                            log_debug("[tur] Created font file ref for Instrument Sans");
                            loaded_files.push(file);
                        }
                        Err(e) => {
                            log_debug(&format!(
                                "[tur] CreateFontFileReference for Instrument Sans failed: {:?}",
                                e
                            ));
                        }
                    }
                }
                log_debug(&format!(
                    "[tur] Play path exists: {}",
                    murmure_path.exists()
                ));
                if murmure_path.exists() {
                    let file_hstr = windows::core::HSTRING::from(murmure_path.as_os_str());
                    match dwrite.CreateFontFileReference(&file_hstr, None) {
                        Ok(file) => {
                            log_debug("[tur] Created font file ref for Le Murmure");
                            loaded_files.push(file);
                        }
                        Err(e) => {
                            log_debug(&format!(
                                "[tur] CreateFontFileReference for Le Murmure failed: {:?}",
                                e
                            ));
                        }
                    }
                }

                if !loaded_files.is_empty() {
                    log_debug(&format!("[tur] Loaded files count: {}", loaded_files.len()));
                    if let Ok(builder) = factory3.CreateFontSetBuilder() {
                        if let Ok(builder1) = builder.cast::<IDWriteFontSetBuilder1>() {
                            for (idx, file) in loaded_files.iter().enumerate() {
                                if let Err(e) = builder1.AddFontFile(file) {
                                    log_debug(&format!(
                                        "[tur] AddFontFile failed for file {}: {:?}",
                                        idx, e
                                    ));
                                } else {
                                    log_debug(&format!(
                                        "[tur] AddFontFile succeeded for file {}",
                                        idx
                                    ));
                                }
                            }
                            if let Ok(font_set) = builder1.CreateFontSet() {
                                if let Ok(col1) =
                                    factory3.CreateFontCollectionFromFontSet(&font_set)
                                {
                                    if let Ok(col) = col1.cast::<IDWriteFontCollection>() {
                                        font_col = Some(col.clone());
                                        log_debug(
                                            "[tur] Loaded custom font collection successfully!",
                                        );
                                        let count = col.GetFontFamilyCount();
                                        log_debug(&format!("[tur] Font family count: {}", count));
                                        for i in 0..count {
                                            match col.GetFontFamily(i) {
                                                Ok(family) => {
                                                    match family.GetFamilyNames() {
                                                        Ok(names) => {
                                                            let count = names.GetCount();
                                                            log_debug(&format!(
                                                                "[tur]   Family {} name count: {}",
                                                                i, count
                                                            ));
                                                            for j in 0..count {
                                                                match names.GetStringLength(j) {
                                                                    Ok(len) => {
                                                                        log_debug(&format!("[tur]     Name {} len: {}", j, len));
                                                                        let mut buf = vec![
                                                                                0u16;
                                                                                (len + 1) as usize
                                                                            ];
                                                                        match names
                                                                            .GetString(j, &mut buf)
                                                                        {
                                                                            Ok(_) => {
                                                                                let name = String::from_utf16_lossy(&buf[..len as usize]);
                                                                                log_debug(&format!("[tur]     Family {} Name {}: '{}'", i, j, name));
                                                                            }
                                                                            Err(e) => {
                                                                                log_debug(&format!("[tur]     GetString err: {:?}", e));
                                                                            }
                                                                        }
                                                                    }
                                                                    Err(e) => {
                                                                        log_debug(&format!("[tur]     GetStringLength err: {:?}", e));
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        Err(e) => {
                                                            log_debug(&format!(
                                                                "[tur]   GetFamilyNames err: {:?}",
                                                                e
                                                            ));
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    log_debug(&format!(
                                                        "[tur]   GetFontFamily err: {:?}",
                                                        e
                                                    ));
                                                }
                                            }
                                        }
                                    }
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
        w!("Instrument Sans"),
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
        w!("Instrument Sans"),
        font_collection_ref,
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
    let text_layout: IDWriteTextLayout =
        dwrite.CreateTextLayout(&label_utf16, &text_fmt, 2000.0, HUD_H)?;
    let mut metrics = DWRITE_TEXT_METRICS::default();
    text_layout.GetMetrics(&mut metrics)?;
    let text_width = metrics.width;

    // Pre-create the X button text layout
    let x_utf16: Vec<u16> = "x".encode_utf16().collect();
    let x_layout: IDWriteTextLayout =
        dwrite.CreateTextLayout(&x_utf16, &x_fmt, X_W, HUD_H)?;

    // Diagnostic font measurement comparison
    if let Ok(segoe_fmt) = dwrite.CreateTextFormat(
        w!("Segoe UI"),
        None,
        DWRITE_FONT_WEIGHT_NORMAL,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        FONT_SIZE,
        w!("en-us"),
    ) {
        if let Ok(segoe_layout) = dwrite.CreateTextLayout(&label_utf16, &segoe_fmt, 2000.0, HUD_H) {
            let mut segoe_metrics = DWRITE_TEXT_METRICS::default();
            if segoe_layout.GetMetrics(&mut segoe_metrics).is_ok() {
                log_debug(&format!(
                    "[tur] Font comparison: Instrument Sans w={:.4}, Segoe UI w={:.4}",
                    text_width, segoe_metrics.width
                ));
            }
        }
    }

    let text_pill_width = PILL_PAD_X + text_width + PILL_PAD_X;
    let hud_width = PILL_START_X + text_pill_width + PILL_GAP_M + X_W + R_PAD;

    // ── D2D solid colour brush ────────────────────────────────────────────────
    let rt: ID2D1RenderTarget = d2d_ctx.cast()?;
    let brush: ID2D1SolidColorBrush = rt.CreateSolidColorBrush(
        &D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        None,
    )?;
    drop(rt);

    // ── WIC PNG logo decode ───────────────────────────────────────────────────
    let logo_bmp = load_logo(&d2d_ctx, dpi).ok();

    log_debug(&format!(
        "[tur] overlay: GPU init OK text_w={text_width:.1} hud_w={hud_width:.1} dpi={dpi}"
    ));

    Ok(GpuState {
        d3d,
        d2d_ctx,
        dwrite,
        swapchain,
        target_bmp: Some(target_bmp),
        dcomp,
        _dcomp_tgt: dcomp_tgt,
        _dcomp_vis: dcomp_vis,
        brush,
        text_layout,
        x_layout,
        _font_collection: font_col,
        logo_bmp,
        text_width,
        hud_width,
        sc_size: (w, h),
    })
}

pub unsafe fn create_d2d_target(
    ctx: &ID2D1DeviceContext,
    sc: &IDXGISwapChain1,
    dpi: f32,
) -> windows::core::Result<ID2D1Bitmap1> {
    let surface: IDXGISurface = sc.GetBuffer(0)?;
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: ManuallyDrop::new(None),
    };
    ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))
}

pub unsafe fn resize_swapchain(
    gpu: &mut GpuState,
    w: u32,
    h: u32,
    dpi: f32,
) -> windows::core::Result<()> {
    // Update the Direct2D device context DPI so text metrics, layout, and
    // vector assets render at the correct scale for the target monitor.
    // This is critical when the browser moves between monitors with different
    // DPIs at the same physical resolution.
    gpu.d2d_ctx.SetDpi(dpi, dpi);
    gpu.d2d_ctx.SetTarget(None); // release D2D ref to the back-buffer bitmap
    gpu.target_bmp = None; // Drop old target bitmap before ResizeBuffers
    gpu.swapchain
        .ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
    let new_target = create_d2d_target(&gpu.d2d_ctx, &gpu.swapchain, dpi)?;
    gpu.d2d_ctx.SetTarget(&new_target);
    gpu.target_bmp = Some(new_target);
    gpu.sc_size = (w, h);
    Ok(())
}
