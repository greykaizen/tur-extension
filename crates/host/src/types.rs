// crates/host/src/types.rs
#![allow(dead_code)]

/// Parsed geometry update from the extension (MEDIA_TARGETS_UPDATE).
#[derive(Debug, Clone)]
pub struct TargetPayload {
    pub element_id: String,
    pub _client_x: i32,
    pub _client_y: i32,
    pub width: i32,
    pub height: i32,
    pub screen_x: i32,
    pub screen_y: i32,
    pub media_url: String,
    pub drag_offset_x: i32,
    pub drag_offset_y: i32,
}

#[derive(Debug, Clone)]
pub struct TargetsUpdate {
    pub tab_id: i32,
    pub _page_url: String,
    pub viewport_screen_x: i32,
    pub viewport_screen_y: i32,
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub _device_pixel_ratio: f64,
    pub targets: Vec<TargetPayload>,
}

#[derive(Debug, Clone)]
pub struct TargetInfo {
    pub element_id: String,
    pub screen_x: i32,
    pub screen_y: i32,
    pub width: i32,
    pub _height: i32,
    pub media_url: String,
    pub drag_offset_x: i32,
    pub drag_offset_y: i32,
}

#[derive(Debug, Clone)]
pub struct CanvasUpdate {
    pub tab_id: i32,
    pub viewport_screen_x: i32,
    pub viewport_screen_y: i32,
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub device_pixel_ratio: f64,
    pub targets: Vec<TargetInfo>,
    pub owner: isize,
    pub is_dark: bool,
}
