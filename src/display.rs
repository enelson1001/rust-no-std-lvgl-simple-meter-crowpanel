//! # LVGL Display and Input Driver for ESP32-S3
//!
//! This module provides the hardware abstraction layer between LVGL (Light and Versatile
//! Graphics Library) and the ESP32-S3 hardware, specifically optimized for DPI/RGB
//! interfaces with external PSRAM framebuffers.
//!
//! ## Memory Architecture
//! To maximize performance and avoid PSRAM XIP (Execute-in-Place) contention, this
//! driver utilizes a hybrid memory strategy:
//!
//! * **Internal DRAM:** The `DRAW_BUF_CELL` (Partial Buffer) and DMA Bounce Buffers
//!   reside in fast internal SRAM. LVGL renders widgets directly into this "scratchpad"
//!   at maximum CPU speed.
//! * **External PSRAM:** The global Framebuffer (`FB1_ADDR`) resides in PSRAM.
//! * **Synchronization:** The `flush_trampoline` performs a row-by-row patch from
//!   the DRAM scratchpad into the PSRAM framebuffer, followed by a hardware cache
//!   write-back to ensure the DPI peripheral sees the updated pixels.
//!
//! ## Key Components
//! * [`Display`]: Manages the registration of the LVGL display driver and the
//!   asynchronous `flush_cb`.
//! * [`Screen`]: A safe wrapper around raw LVGL object pointers for screen management.
//! * **Input Integration:** Provides an atomic-backed trampoline for touchscreen
//!   data, allowing high-priority I2C tasks to update touch coordinates without
//!   blocking the UI thread.
//!
//! ## Safety and Performance
//! * All callbacks are marked `#[esp_hal::ram]` to ensure they are placed in IRAM,
//!   preventing "Cache Disabled" panics during Flash/PSRAM write operations.
//! * Uses atomic operations (`AtomicI16`, `AtomicBool`) for thread-safe communication
//!   between the touch sensor task and the LVGL engine.

use alloc::boxed::Box;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicI16, Ordering};
use static_cell::StaticCell;

use crate::lvgl_sys;

extern crate alloc;

use crate::{FB1_ADDR, PARTIAL_BUF_SIZE, SCREEN_WIDTH};

static TOUCH_X: AtomicI16 = AtomicI16::new(0);
static TOUCH_Y: AtomicI16 = AtomicI16::new(0);
pub static TOUCH_PRESSED: AtomicBool = AtomicBool::new(false);

// Ensures LVGL doesn't miss a quick tap
static TOUCH_UNREAD_PRESS: AtomicBool = AtomicBool::new(false);

pub struct Screen {
    pub(crate) raw: NonNull<lvgl_sys::lv_obj_t>,
}

impl Screen {
    /// Helper to get the raw pointer for LVGL C-API calls
    pub fn as_ptr(&self) -> *mut lvgl_sys::lv_obj_t {
        self.raw.as_ptr()
    }
}

pub struct Display {
    pub disp: NonNull<lvgl_sys::lv_disp_t>,
}

impl Display {
    pub fn register_partial(hor_res: u32, ver_res: u32) -> Self {
        // Using a static array for zero-latency access and ensures this lands in Internal RAM
        static DRAW_BUF_CELL: StaticCell<[u16; PARTIAL_BUF_SIZE]> = StaticCell::new();
        let draw_buf_ref = DRAW_BUF_CELL.init([0u16; PARTIAL_BUF_SIZE]);

        let mut draw_buf = Box::new(unsafe { core::mem::zeroed::<lvgl_sys::lv_disp_draw_buf_t>() });

        unsafe {
            lvgl_sys::lv_disp_draw_buf_init(
                draw_buf.as_mut(),
                draw_buf_ref.as_mut_ptr() as *mut _,
                core::ptr::null_mut(), // No second buffer needed in partial mode
                PARTIAL_BUF_SIZE as u32,
            );
        }

        let leaked_draw_buf: &'static mut lvgl_sys::lv_disp_draw_buf_t = Box::leak(draw_buf);

        // Setup the Driver
        let mut disp_drv = Box::new(unsafe {
            let mut inner = MaybeUninit::<lvgl_sys::lv_disp_drv_t>::uninit();
            lvgl_sys::lv_disp_drv_init(inner.as_mut_ptr());
            inner.assume_init()
        });

        disp_drv.hor_res = hor_res as i16;
        disp_drv.ver_res = ver_res as i16;
        disp_drv.draw_buf = leaked_draw_buf as *mut _;

        // --- PARTIAL MODE SETTINGS ---
        disp_drv.set_full_refresh(0); // MUST be 0 for partial mode
        disp_drv.set_direct_mode(0); // MUST be 0 for partial mode

        disp_drv.flush_cb = Some(flush_trampoline);

        let disp = unsafe {
            let ptr = lvgl_sys::lv_disp_drv_register(disp_drv.as_mut() as *mut _);
            lvgl_sys::lv_disp_set_default(ptr);
            NonNull::new(ptr).expect("LVGL Display Register Failed")
        };

        Box::leak(disp_drv);
        Self { disp }
    }

    pub fn register_touchscreen(&self) {
        let mut indev_drv = Box::new(unsafe {
            let mut inner = MaybeUninit::<lvgl_sys::lv_indev_drv_t>::uninit();
            lvgl_sys::lv_indev_drv_init(inner.as_mut_ptr());
            inner.assume_init()
        });

        indev_drv.type_ = lvgl_sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER;
        indev_drv.read_cb = Some(touch_read_trampoline);

        //indev_drv.gesture_limit = 10; // Pixels to move before it's a drag
        //indev_drv.gesture_min_velocity = 20;

        //indev_drv.scroll_limit = 10; // Pixels before scrolling starts
        //indev_drv.scroll_throw = 10; // Reduce the momentum

        unsafe {
            lvgl_sys::lv_indev_drv_register(indev_drv.as_mut() as *mut _);
        }

        // Keep the driver alive forever
        Box::leak(indev_drv);
    }

    /// Returns the currently active screen for this display.
    pub fn get_scr_act(&self) -> Screen {
        unsafe {
            // Most LVGL versions use this underlying function
            let ptr = lvgl_sys::lv_disp_get_scr_act(self.disp.as_ptr() as *mut _);
            Screen {
                raw: NonNull::new(ptr).expect("Active screen null"),
            }
        }
    }
}

// This is called once per frame by LVGL
#[esp_hal::ram]
unsafe extern "C" fn flush_trampoline(
    disp_drv: *mut lvgl_sys::lv_disp_drv_t,
    area: *const lvgl_sys::lv_area_t,
    color_p: *mut lvgl_sys::lv_color_t,
) {
    unsafe {
        let area = &*area;
        let fb_ptr = FB1_ADDR.load(Ordering::Relaxed) as *mut u16;

        // Width and height of the dirty patch
        let patch_width = (area.x2 - area.x1 + 1) as usize;
        let patch_height = (area.y2 - area.y1 + 1) as usize;
        let src_pixels = color_p as *const u16;

        // Row-by-row patch into PSRAM
        for y in 0..patch_height {
            let dst_row_start = ((area.y1 as usize + y) * SCREEN_WIDTH) + area.x1 as usize;
            let src_row_start = y * patch_width;

            core::ptr::copy_nonoverlapping(
                src_pixels.add(src_row_start),
                fb_ptr.add(dst_row_start),
                patch_width,
            );

            // Cache flush specifically for the modified row segment
            // Always flush in multiples of 32 bytes or the full row for safety
            rom_Cache_WriteBack_Addr(fb_ptr.add(dst_row_start) as u32, (patch_width * 2) as u32);
        }

        // Tell LVGL it can start preparing the next frame
        lvgl_sys::lv_disp_flush_ready(disp_drv);
    }
}

#[esp_hal::ram]
unsafe extern "C" fn touch_read_trampoline(
    _indev_drv: *mut lvgl_sys::lv_indev_drv_t,
    data: *mut lvgl_sys::lv_indev_data_t,
) {
    unsafe {
        let data = &mut *data;

        // Fill the LVGL data structure from our atomics
        data.point.x = TOUCH_X.load(Ordering::Relaxed);
        data.point.y = TOUCH_Y.load(Ordering::Relaxed);

        let is_pressed = TOUCH_PRESSED.load(Ordering::Relaxed);
        let unread_press = TOUCH_UNREAD_PRESS.swap(false, Ordering::Relaxed);

        // If we have an unread press, tell LVGL it's pressed even if
        // the hardware currently says it's released.
        data.state = if is_pressed || unread_press {
            lvgl_sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
        } else {
            lvgl_sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
        };
    }
}

// Call this from your main loop/I2C task when the touch hardware reports data
#[esp_hal::ram]
pub fn update_touch_data(x: i16, y: i16, pressed: bool) {
    TOUCH_X.store(x, Ordering::Relaxed);
    TOUCH_Y.store(y, Ordering::Relaxed);
    TOUCH_PRESSED.store(pressed, Ordering::Relaxed);

    if pressed {
        TOUCH_UNREAD_PRESS.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" {
    // This tells the Rust compiler that the function is defined elsewhere
    pub fn rom_Cache_WriteBack_Addr(addr: u32, size: u32);
}
