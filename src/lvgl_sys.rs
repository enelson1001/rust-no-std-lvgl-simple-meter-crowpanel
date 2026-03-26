// src/lvgl_sys.rs
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::redundant_static_lifetimes)]
// Add these to silence the "useless transmute" and other generated noise
#![allow(clippy::all)]
#![allow(clippy::pedantic)]
#![allow(clippy::nursery)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
