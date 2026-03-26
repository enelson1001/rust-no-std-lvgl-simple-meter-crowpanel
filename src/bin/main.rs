//! Code based off the example code in lvgl v8.4 repository, examples/widgets/meter/lv_example_meter_1.c
//!
//! The application is designed for an ESP32-S3 based device with a display and a GT911 touchscreen
//! controller. It utilizes the `embassy` async runtime for concurrent operations.
//!
//! # Core Functionality:
//!
//! - **Hardware Initialization**: Sets up the system clock, PSRAM, heap allocators, and peripherals
//!   including I2C and the LCD/CAM DMA controller. It uses a `STC8H1K28` microcontroller to control
//!   display backlight and GT911 reset.
//!
//! - **Display Driver**: A high-priority `embassy` task (`display_engine_task`) manages the
//!   asynchronous LCD refresh. It utilizes a DMA bounce buffer to continuously stream
//!   framebuffer data from PSRAM to the display hardware. This architecture ensures
//!   jitter-free rendering by offloading the pixel-clock timing to the DMA controller.
//!
//! - **LVGL Graphics**: The application leverages `lvgl` for the UI, utilizing a small
//!   SRAM-based partial draw buffer to minimize internal bus contention. The `display_engine_task`
//!   coordinates with the LVGL engine to ensure that while the DMA is actively refreshing
//!   the panel from PSRAM, LVGL renders its next "dirty" chunk into the partial buffer.
//!   During the vertical blanking interval (VBlank), these updates are merged into the
//!   main PSRAM framebuffer, preventing visual tearing and blocking.
//!
//! - **Touch Input**: A dedicated task (`read_touchscreen_task`) polls the GT911 touchscreen
//!   controller over I2C. Touch events are passed to `lvgl` using atomic variables to ensure
//!   thread-safe communication without blocking the UI thread. The touch data is updated in
//!   a way that allows the high-priority I2C task to signal touch events without needing to
//!   acquire locks or directly interact with LVGL's internal data structures
//!
//! - **Concurrency**: The application is structured around several asynchronous tasks managed by
//!   the `embassy` executor:
//!   - `display_engine_task`: High-priority task for display rendering.
//!   - `lvgl_tick_task`: Provides a periodic tick required by `lvgl`.
//!   - `lvgl_task_handler_task`: Calls the main `lvgl` processing loop.
//!   - `read_touchscreen_task`: Handles touch input polling.
//!   - `gui_handler_task`: Manages the application's UI logic, not used in this example.
//!

#![no_std]
#![no_main]

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};

use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::{
    Async, Blocking,
    gpio::Level,
    i2c::master::{Config, I2c},
    interrupt::{Priority, software::SoftwareInterruptControl},
    lcd_cam::{
        LcdCam,
        lcd::{
            ClockMode, Phase, Polarity,
            dpi::{self, Dpi, Format, FrameTiming},
        },
    },
    time::Rate,
    timer::timg::TimerGroup,
};

use esp_hal::xtensa_lx;

//use esp_hal::gpio::{Output, OutputConfig};

use esp_rtos::embassy::InterruptExecutor;
use log::*;

use alloc::{boxed::Box, vec};
use core::sync::atomic::Ordering;
use static_cell::StaticCell;

use gt911::Gt911;

use embedded_hal_async::i2c::I2c as _;

use rust_no_std_lvgl_simple_meter_crowpanel::{
    FB1_ADDR, FRAMEBUFFER_SIZE, SCREEN_HEIGHT, SCREEN_WIDTH,
    bounce_buffer_dma::{BounceBufferDma, fill_bounce_buffer},
    display::{Display, TOUCH_PRESSED, update_touch_data},
    lv_glue,
    simple_meter_page::*,
};

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

// --- STATIC STORAGE ---
static I2C_BUS: StaticCell<Mutex<CriticalSectionRawMutex, I2c<'static, Async>>> = StaticCell::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    info!("A. Starting main");

    let peripherals = esp_hal::init(
        esp_hal::Config::default()
            .with_cpu_clock(esp_hal::clock::CpuClock::max())
            .with_psram(esp_hal::psram::PsramConfig {
                flash_frequency: esp_hal::psram::FlashFreq::FlashFreq80m,
                ram_frequency: esp_hal::psram::SpiRamFreq::Freq80m, // MUST BE Freq80m
                core_clock: Some(
                    esp_hal::psram::SpiTimingConfigCoreClock::SpiTimingConfigCoreClock160m,
                ),
                execute_from_psram: true,
                ..Default::default()
            }),
    );

    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // See the heap usage at the beginning of program.
    print_heap_stats();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // Time to allow stc8h1k28 to startup
    Timer::after(Duration::from_millis(200)).await;

    // Test pin for timing
    //let tp19 = Output::new(peripherals.GPIO19, Level::Low, OutputConfig::default());
    //let tp20 = Output::new(peripherals.GPIO20, Level::Low, OutputConfig::default());

    // Setup up I2C bus
    info!("B. Setting up I2C bus");
    let mut i2c = I2c::new(
        peripherals.I2C0,
        Config::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO15)
    .with_scl(peripherals.GPIO16)
    .into_async();

    info!("--- Starting I2C Scan ---");
    for address in 0x01..0x7f {
        // We attempt a 0-byte write to see if the device ACKs
        if i2c.write(address, &[]).is_ok() {
            info!("Found device at address: 0x{:02X}", address);
        }
    }
    info!("--- I2C Scan Complete ---");

    // Create I2c shared bus
    let i2c_bus = I2C_BUS.init(Mutex::new(i2c));

    // Create the I2C devices that share the bus
    let i2c_touchscreen = I2cDevice::new(i2c_bus);
    let mut i2c_stc8h1k28 = I2cDevice::new(i2c_bus);

    info!("C. Setting up the IO expander STC8H");
    // Send commands to STC8H1K28 to turn on speaker, set screen backlight to max
    let _ = i2c_stc8h1k28.write(0x30, &[248]).await; // Speaker ON
    let _ = i2c_stc8h1k28.write(0x30, &[0]).await; // Backlight MAX //150

    // Send commands to STC8H1K28 to turn on buzzer for one second and then turn off
    // Good test to see if you can communicate with the STC8H1K28
    //let _ = i2c_stc8h1k28.write(0x30, &[246]).await; // Buzzer ON
    //Timer::after(Duration::from_millis(1000)).await;
    //let _ = i2c_stc8h1k28.write(0x30, &[247]).await; // Buzzer OFF

    // Send command to STC8H1K28 to reset GT911 - Pulses TP_RST signal low for 100ms
    // and allow time for the GT911 to reset
    let _ = i2c_stc8h1k28.write(0x30, &[250]).await;
    Timer::after(Duration::from_millis(100)).await;

    // The following timings given the DPI peripherals has the LCD display being refreshed every 23.12ms or 43FPS
    // The time to draw one horizontal line = 816/21MHz = 38.857us
    // Time to draw screen = 595 * 38.857us = 23.12ms
    // Vertical front porch time = 103 * 38.857us = 4.0ms
    // Vertical back porch time = (595 - 103 ) * 38.857us = 466us
    info!("D. Creating dpi interface");
    let tx_channel = peripherals.DMA_CH2;
    let lcd_cam = LcdCam::new(peripherals.LCD_CAM);

    let dpi_config = dpi::Config::default()
        .with_frequency(Rate::from_mhz(21))
        .with_clock_mode(ClockMode {
            polarity: Polarity::IdleLow,
            phase: Phase::ShiftHigh,
        })
        .with_format(Format {
            enable_2byte_mode: true,
            ..Default::default()
        })
        .with_timing(FrameTiming {
            horizontal_active_width: 800,
            horizontal_total_width: 816,
            horizontal_blank_front_porch: 8,
            hsync_width: 4,
            vertical_active_height: 480,
            vertical_total_height: 595,
            vertical_blank_front_porch: 103,
            vsync_width: 4,
            hsync_position: 0,
        })
        .with_vsync_idle_level(Level::High)
        .with_hsync_idle_level(Level::High)
        .with_de_idle_level(Level::Low)
        .with_hs_blank_en(true);

    let dpi = Dpi::new(lcd_cam.lcd, tx_channel, dpi_config)
        .unwrap()
        .with_pclk(peripherals.GPIO39)
        .with_vsync(peripherals.GPIO41)
        .with_hsync(peripherals.GPIO40)
        .with_de(peripherals.GPIO42)
        // Blue
        .with_data0(peripherals.GPIO21)
        .with_data1(peripherals.GPIO47)
        .with_data2(peripherals.GPIO48)
        .with_data3(peripherals.GPIO45)
        .with_data4(peripherals.GPIO38)
        // Green
        .with_data5(peripherals.GPIO9)
        .with_data6(peripherals.GPIO10)
        .with_data7(peripherals.GPIO11)
        .with_data8(peripherals.GPIO12)
        .with_data9(peripherals.GPIO13)
        .with_data10(peripherals.GPIO14)
        // Red
        .with_data11(peripherals.GPIO7)
        .with_data12(peripherals.GPIO17)
        .with_data13(peripherals.GPIO18)
        .with_data14(peripherals.GPIO3) // elecrow crowpanel v1.3
        .with_data15(peripherals.GPIO46);

    info!("E. Setting up High Priority Executor");
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    static EXECUTOR: StaticCell<InterruptExecutor<2>> = StaticCell::new();
    let executor = EXECUTOR.init(InterruptExecutor::new(sw_int.software_interrupt2));
    let high_prio_spawner = executor.start(Priority::Priority3);

    info!("F. Setting up lvgl");
    lv_glue::initialize_lgvl();

    // Allocate framebuffer in PSRAM, Box::leak(vec![...]) ensures this lands in the PSRAM heap
    let fb1_raw: &'static mut [u8] = Box::leak(vec![0x00u8; FRAMEBUFFER_SIZE].into_boxed_slice());
    let fb1_ptr = fb1_raw.as_mut_ptr();
    FB1_ADDR.store(fb1_ptr, Ordering::Relaxed);

    // For testing; won't be displayed if using lvgl page because lvgl page will overwrite
    // But if we want test to see if display is working we can comment out the following lvgl
    // lines up to and including lv_glue::lvgl_task_handler()
    #[allow(clippy::unusual_byte_groupings)]
    let blue_pixel: u16 = 0b00000_000000_11111;
    let blue_bytes = blue_pixel.to_le_bytes();
    for i in 0..(800 * 480) {
        fb1_raw[i * 2..i * 2 + 2].copy_from_slice(&blue_bytes);
    }

    let display = Display::register_partial(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
    display.register_touchscreen();

    info!("G. Creating GUI page");
    let _simple_meter_page = SimpleMeterPage::new(display.get_scr_act());

    // Preload lvgl with the initial screen to ensure framebuffer is populated before display engine starts
    lv_glue::lvgl_task_handler();

    info!("H. Spawning tasks.");
    spawner.spawn(lvgl_tick_task()).unwrap();
    spawner.spawn(lvgl_task_handler_task()).unwrap();
    spawner
        .spawn(read_touchscreen_task(i2c_touchscreen))
        .unwrap();
    //spawner.spawn(gui_handler_task()).unwrap();

    // Create the DMA bounce buffers located in SRAM for the display engine
    let bb_dma = BounceBufferDma::init();

    info!("I. Starting display engine");
    high_prio_spawner.must_spawn(display_engine_task(dpi, bb_dma));

    // See how much heap was actually used in this program.
    print_heap_stats();

    // See lvgl memory usage
    lv_glue::print_lv_mem_info();

    loop {
        info!("Hello from main loop!");
        Timer::after(Duration::from_secs(10)).await;
    }
}

#[embassy_executor::task]
async fn display_engine_task(mut dpi: Dpi<'static, Blocking>, mut bb_dma: BounceBufferDma) {
    // If clock is 240MHz, this is 466 * 240.
    // If clock is 160MHz, it adjusts automatically.
    // It takes 466us to send one chunk or 12 lines to the lcd dsiplay
    let chunk_cycles = 466 * esp_hal::clock::Clocks::get().cpu_clock.as_mhz();

    // --- Initial Fill, Prepare Chunk 0 (Lines 0-11)  ---
    fill_bounce_buffer(0);

    loop {
        // Start hardware transfer
        let transfer = dpi.send(false, bb_dma).map_err(|e| e.0).unwrap();

        // ---- VERTICAL FRONT PORCH BLANKING PERIOD (4 milliseconds) ----
        Timer::after(Duration::from_micros(3950)).await;

        // Chunk Loop, one chunk equals 12 lines
        let mut current_chunk = 0;
        while current_chunk < 40 {
            let start_of_chunk = xtensa_lx::timer::get_cycle_count();

            // While the previous chunk is in flight to the display fill the next chunk
            let next_chunk = current_chunk + 1;
            fill_bounce_buffer(next_chunk);

            // --- Precision time delay (blocking) ---
            // Wait for DMA to eat the previous 12 lines.
            // The time it takes to draw 12 horizontal lines = 12 * 38.857us = 466us
            // The time it takes to fill bounce buffer = copy 19200 bytes of data from psram to sram is 315us
            // The theoretical time to wait is 466us - 315us = 151us
            // Must use blocking delay so no code is executed or else you will get tearing.
            while xtensa_lx::timer::get_cycle_count().wrapping_sub(start_of_chunk) <= chunk_cycles {
                core::hint::spin_loop();
            }

            current_chunk += 1;
        }

        // ---- VERTICAL BACK PORCH BLANKING PERIOD (466us) ----
        // Prepare Chunk 0 the next frame during vertical backporch
        fill_bounce_buffer(0);

        // Recover ownership
        (_, dpi, bb_dma) = transfer.wait();
    }
}

#[embassy_executor::task]
pub async fn lvgl_tick_task() {
    loop {
        Timer::after(Duration::from_millis(5)).await;
        lv_glue::lvgl_tick_inc(5);
    }
}

#[embassy_executor::task]
pub async fn lvgl_task_handler_task() {
    loop {
        let sleep_ms = lv_glue::lvgl_task_handler();

        // Don't sleep more than 4ms and at least 1ms
        let actual_sleep = sleep_ms.clamp(10, 16);
        Timer::after(Duration::from_millis(actual_sleep as u64)).await;
    }
}

#[embassy_executor::task]
async fn read_touchscreen_task(
    mut i2c: I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>,
) {
    info!("Attempting to initialize GT911");
    let touch = Gt911::new(0x5d);
    let mut buf = [0u8; 8];

    // Note the init clears the status register.
    if let Err(e) = touch.init(&mut i2c, &mut buf).await {
        error!("Error initializing touch: {:?}", e);
        return;
    }

    info!("GT911 Initialized, waiting for touch...");

    loop {
        match touch.get_touch(&mut i2c, &mut buf).await {
            Ok(Some(point)) => {
                // A touch is active: update coordinates and set pressed to true
                update_touch_data(point.x as i16, point.y as i16, true);
            }
            Ok(None) => {
                // No touch active: set pressed to false
                // We don't update X/Y so LVGL remembers the last position
                TOUCH_PRESSED.store(false, Ordering::Relaxed);
            }
            Err(_e) => {
                // Usually Error::NotReady if no new data, or a real I2C error
                // We treat this as a release to be safe
                //TOUCH_PRESSED.store(false, Ordering::Relaxed);
            }
        }

        // Poll rate: ~60Hz is plenty for smooth UI interaction
        Timer::after(Duration::from_millis(16)).await;
    }
}

#[embassy_executor::task]
pub async fn gui_handler_task() {
    loop {
        // Nothing to do for this application
        embassy_time::Timer::after_millis(100).await;
    }
}

fn print_heap_stats() {
    let used = esp_alloc::HEAP.used();
    let free = esp_alloc::HEAP.free();
    let total = used + free;

    esp_println::println!(
        "Heap Info: Used: {} | Free: {} | Total: {}",
        used,
        free,
        total
    );
}
