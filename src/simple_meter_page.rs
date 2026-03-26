use crate::{display::Screen, lvgl_sys};
use lvgl_sys::*;

// --- CONSTANTS ---
const FALSE: lvgl_sys::bool_ = 0;
const _TRUE: lvgl_sys::bool_ = 1; // made unsed to avoid "unused" warning, but LV_TRUE is not actually used in this example
//const LV_COLOR_WHITE: lv_color_t = color_from_rgb(0xFF, 0xFF, 0xFF);
const LV_COLOR_BLACK: lv_color_t = color_from_rgb(0x00, 0x00, 0x00);
const PALETTE_GREY: lv_palette_t = lv_palette_t_LV_PALETTE_GREY;

/*******************************************************************************************
 *  STATICS STORAGE FOR LVGL OBJECTS
 ******************************************************************************************/
static mut METER_PTR: *mut lv_obj_t = core::ptr::null_mut();

pub struct SimpleMeterPage {}

impl SimpleMeterPage {
    pub fn new(screen: Screen) -> Self {
        unsafe {
            // --- Get pointer to the screen ---
            let screen_ptr = screen.as_ptr();

            // --- Style the Screen ---
            lv_obj_set_style_bg_color(screen_ptr, color_from_rgb(53, 56, 57), 0);
            lv_obj_set_style_bg_opa(screen_ptr, 255, 0);

            // --- Create Meter ---
            let meter = lv_meter_create(screen_ptr);
            METER_PTR = meter; // Store globally for the animation callback
            lv_obj_set_size(meter, 200, 200);
            lv_obj_align(meter, LV_ALIGN_CENTER as u8, 0, 0);

            // --- Add a scale ---
            let scale = lv_meter_add_scale(meter);
            lv_meter_set_scale_ticks(meter, scale, 41, 2, 10, lv_palette_main(PALETTE_GREY));
            lv_meter_set_scale_major_ticks(meter, scale, 8, 4, 15, LV_COLOR_BLACK, 10);

            // --- Add  a blue arc to the start ---
            let mut indic = lv_meter_add_arc(meter, scale, 3, color_from_rgb(0, 0, 255), 0);
            lv_meter_set_indicator_start_value(meter, indic, 0);
            lv_meter_set_indicator_end_value(meter, indic, 20);

            // --- Make tick lines blue at the start of the scale ---
            indic = lv_meter_add_scale_lines(
                meter,
                scale,
                color_from_rgb(0, 0, 255),
                color_from_rgb(0, 0, 255),
                FALSE,
                0,
            );
            lv_meter_set_indicator_start_value(meter, indic, 0);
            lv_meter_set_indicator_end_value(meter, indic, 20);

            // --- Add a red arc to the end ---
            indic = lv_meter_add_arc(meter, scale, 3, color_from_rgb(255, 0, 0), 0);
            lv_meter_set_indicator_start_value(meter, indic, 80);
            lv_meter_set_indicator_end_value(meter, indic, 100);

            // --- Make tick lines red at the end of the scale ---
            indic = lv_meter_add_scale_lines(
                meter,
                scale,
                color_from_rgb(255, 0, 0),
                color_from_rgb(255, 0, 0),
                FALSE,
                0,
            );
            lv_meter_set_indicator_start_value(meter, indic, 80);
            lv_meter_set_indicator_end_value(meter, indic, 100);

            // --- Add a needle line indicator ---
            indic = lv_meter_add_needle_line(meter, scale, 4, lv_palette_main(PALETTE_GREY), -10);

            // --- Create an animation to set the value of the needle ---
            let mut anim = core::mem::MaybeUninit::<lv_anim_t>::zeroed().assume_init();
            lv_anim_init(&mut anim);
            anim.exec_cb = Some(set_value); // Direct field access
            anim.var = indic as *mut core::ffi::c_void;
            anim.start_value = 0;
            anim.end_value = 100;
            anim.time = 2000;
            anim.playback_time = 500;
            anim.repeat_cnt = LV_ANIM_REPEAT_INFINITE as u16;
            lv_anim_start(&anim);

            Self {}
        }
    }
}

// C-style callback for LVGL animation
#[esp_hal::ram]
unsafe extern "C" fn set_value(var: *mut core::ffi::c_void, value: i32) {
    unsafe {
        let indic = var as *mut lv_meter_indicator_t;
        if !METER_PTR.is_null() {
            lv_meter_set_indicator_value(METER_PTR, indic, value);
        }
    }
}

/*******************************************************************************************
 *  LVGL Helpers
 ******************************************************************************************/
#[inline(always)]
const fn color_from_rgb(r: u8, g: u8, b: u8) -> lv_color_t {
    // Scale 8-bit colors down to 5-6-5 bit depths
    // Red:   8 bits -> 5 bits (r >> 3)
    // Green: 8 bits -> 6 bits (g >> 2)
    // Blue:  8 bits -> 5 bits (b >> 3)
    let r5 = (r >> 3) as u16;
    let g6 = (g >> 2) as u16;
    let b5 = (b >> 3) as u16;

    // Manually pack the RGB565 bits:
    // Red   is bits 11-15
    // Green is bits 5-10
    // Blue  is bits 0-4
    let full = (r5 << 11) | (g6 << 5) | b5;

    lv_color_t { full }
}
