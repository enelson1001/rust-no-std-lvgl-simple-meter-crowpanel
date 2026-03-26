#![no_std]
// Modules
pub mod bounce_buffer_dma;
pub mod display;
pub mod lv_glue;
pub mod lvgl_sys;
pub mod simple_meter_page;

// Constants
pub const SCREEN_WIDTH: usize = 800;
pub const SCREEN_HEIGHT: usize = 480;
pub const BYTES_PER_PIXEL: usize = 2; // Example for RGB565
pub const FRAMEBUFFER_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL;

/// Number of rows rendered by LVGL into SRAM before patching to PSRAM.
/// Increasing this reduces CPU overhead but consumes more internal DRAM.
pub const LVGL_PARTIAL_LINES: usize = 48;

/// Total pixels in the partial buffer
pub const PARTIAL_BUF_SIZE: usize = SCREEN_WIDTH * LVGL_PARTIAL_LINES;

// --- STATIC STORAGE ---
use core::sync::atomic::AtomicPtr;
pub static FB1_ADDR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
