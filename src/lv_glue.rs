use crate::lvgl_sys;
use lvgl_sys::*;

use log::*;

pub fn initialize_lgvl() {
    unsafe {
        lvgl_sys::lv_init();
    }
}

pub fn lvgl_tick_inc(duration: u32) {
    unsafe {
        lv_tick_inc(duration);
    }
}

pub fn lvgl_task_handler() -> u32 {
    unsafe { lv_timer_handler() }
}

pub fn print_lv_mem_info() {
    unsafe {
        let mut mon = core::mem::MaybeUninit::<lv_mem_monitor_t>::uninit();
        lv_mem_monitor(mon.as_mut_ptr());
        let mon = mon.assume_init();

        info!(
            "LVGL Mem: total: {}, free: {}, used: {}% (frag: {}%)",
            mon.total_size, mon.free_size, mon.used_pct, mon.frag_pct
        );
        info!("-------------------------------------");
    }
}
