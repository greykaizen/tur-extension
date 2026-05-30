// crates/host/src/types.rs
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TargetStatus {
    Pending,
    Resolving,
    Ready,
}

impl Default for TargetStatus {
    fn default() -> Self {
        TargetStatus::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatInfo {
    pub label: String,
    pub video_url: String,
    pub audio_url: String,
    pub resolution: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetPayload {
    pub element_id: String,
    pub client_x: i32,
    pub client_y: i32,
    pub width: i32,
    pub height: i32,
    pub screen_x: i32,
    pub screen_y: i32,
    pub media_url: String,
    pub drag_offset_x: i32,
    pub drag_offset_y: i32,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub status: TargetStatus,
    #[serde(default)]
    pub formats: Vec<FormatInfo>,
    #[serde(default)]
    pub cookie: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsUpdate {
    pub tab_id: i32,
    pub page_url: String,
    pub referer: String,
    pub user_agent: String,
    pub viewport_screen_x: i32,
    pub viewport_screen_y: i32,
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub device_pixel_ratio: f64,
    pub targets: Vec<TargetPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInfo {
    pub element_id: String,
    pub screen_x: i32,
    pub screen_y: i32,
    pub width: i32,
    pub _height: i32,
    pub media_url: String,
    pub drag_offset_x: i32,
    pub drag_offset_y: i32,
    pub duration: f64,
    pub status: TargetStatus,
    pub formats: Vec<FormatInfo>,
    pub cookie: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub referer: String,
    pub user_agent: String,
}
