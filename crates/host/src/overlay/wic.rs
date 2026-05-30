// crates/host/src/overlay/wic.rs

use std::mem::ManuallyDrop;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap1, ID2D1DeviceContext, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_PROPERTIES1,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

const LOGO_PNG: &[u8] = include_bytes!("../../../../extension/src/icons/32x32.png");

pub unsafe fn load_logo(
    ctx: &ID2D1DeviceContext,
    dpi: f32,
) -> windows::core::Result<ID2D1Bitmap1> {
    let factory: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let stream: IWICStream = factory.CreateStream()?;
    stream.InitializeFromMemory(LOGO_PNG)?;
    let decoder =
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
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
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
        colorContext: ManuallyDrop::new(None),
    };
    ctx.CreateBitmapFromWicBitmap(&converter, Some(&props))
}
