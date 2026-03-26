//! # DMA Bounce Buffer Driver for DPI/RGB LCD
//!
//! This module implements a high-performance DMA (Direct Memory Access) pipeline
//! specifically designed to feed an RGB/DPI display peripheral from PSRAM while
//! bypassing PSRAM latency issues.
//!
//! ## The Problem: PSRAM Contention
//! The ESP32-S3 LCD peripheral requires a jitter-free stream of data. Reading
//! directly from External PSRAM often fails to meet timing requirements due to
//! cache misses or bus contention, resulting in visible screen tearing or "underflow."
//!
//! ## The Solution: Double-Buffered "Bounce" Strategy
//! This driver uses two small, 32-byte aligned SRAM buffers ([`BOUNCE_BUF_1`] and
//! [`BOUNCE_BUF_2`]). While the DMA engine is reading one buffer to feed the screen,
//! the CPU/DMA is filling the other buffer from the PSRAM framebuffer.
//!
//! ### Pipeline Mechanism:
//! 1. **Descriptor Chain:** A static chain of [`TOTAL_DESCRIPTORS`] (200 in this config)
//!    is created at startup. These descriptors point alternately to the two SRAM buffers.
//! 2. **Ping-Ponging:** Each "chunk" represents [`BOUNCE_LINES`] (12 lines) of the display.
//!    The DMA controller handles the transfer of these chunks to the LCD hardware.
//! 3. **Manual Refill:** The function [`fill_bounce_buffer`] is called (typically by a
//!    high-priority interrupt or task) to copy the next chunk of pixel data from
//!    PSRAM to the SRAM buffer that is currently idle.
//!
//! ## Memory Configuration
//! * **Bounce Buffers:** Placed in internal `.data` section (DRAM) for zero-latency
//!   DMA access.
//! * **Descriptors:** 200 descriptors manage the "infinite" linear loop of the frame.
//! * **Alignment:** Buffers are 32-byte aligned to match the S3's cache-line and DMA
//!   requirements.
//!
//! ## Performance Constants
//! * **Lines per Chunk:** 12
//! * **DRAM Usage:** ~40 KiB (2x 19.2 KiB buffers + descriptors)
//! * **Cache Coherency:** Uses `rom_Cache_WriteBack_Addr` to ensure CPU writes land
//!   in memory before DMA access.

use core::ptr::addr_of_mut;
use core::sync::atomic::Ordering;
use esp_hal::{
    dma::{BurstConfig, DmaDescriptor, DmaTxBuffer, Owner, Preparation, TransferDirection},
    ram,
};

use crate::{BYTES_PER_PIXEL, FB1_ADDR, SCREEN_HEIGHT, SCREEN_WIDTH};

// --- CONSTANTS ---
pub const BOUNCE_LINES: usize = 12;
pub const BOUNCE_SIZE: usize = SCREEN_WIDTH * BOUNCE_LINES * BYTES_PER_PIXEL; // 6400 bytes

// 19200 / 5 = 3840. This is perfectly 32-byte aligned (3840 / 32 = 120)
pub const DESCRIPTORS_PER_BUFFER: usize = 5;
pub const CHUNK_SIZE: usize = BOUNCE_SIZE / DESCRIPTORS_PER_BUFFER; // 3840
pub const NUM_CHUNKS_IN_FRAME: usize = SCREEN_HEIGHT / BOUNCE_LINES; // 40
pub const TOTAL_DESCRIPTORS: usize = DESCRIPTORS_PER_BUFFER * NUM_CHUNKS_IN_FRAME; // 200

// --- STATIC STORAGE ---
#[repr(C, align(32))]
pub struct BufferWrapper(pub [u8; BOUNCE_SIZE]);

#[repr(C, align(4))]
struct DescriptorWrapper([DmaDescriptor; TOTAL_DESCRIPTORS]);

#[unsafe(link_section = ".data")]
pub static mut BOUNCE_BUF_1: BufferWrapper = BufferWrapper([0u8; BOUNCE_SIZE]);

#[unsafe(link_section = ".data")]
pub static mut BOUNCE_BUF_2: BufferWrapper = BufferWrapper([0u8; BOUNCE_SIZE]);

#[unsafe(link_section = ".data")]
static mut TX_DESCRIPTORS: DescriptorWrapper =
    DescriptorWrapper([DmaDescriptor::EMPTY; TOTAL_DESCRIPTORS]);

// --- DRIVER STRUCT ---
pub struct BounceBufferDma {
    descriptors: &'static mut [DmaDescriptor],
    _buffer_area: &'static mut [u8],
}

impl BounceBufferDma {
    /// Create the descriptor chain that uses 2 bounce buffers, each bounce buffer holds 12 lines or 19200 bytes.
    pub fn init() -> Self {
        unsafe {
            let desc_ptr = addr_of_mut!(TX_DESCRIPTORS.0);
            let bb1_ptr = addr_of_mut!(BOUNCE_BUF_1.0) as *mut u8;
            let bb2_ptr = addr_of_mut!(BOUNCE_BUF_2.0) as *mut u8;

            for i in 0..TOTAL_DESCRIPTORS {
                let is_bb2 = !(i / DESCRIPTORS_PER_BUFFER).is_multiple_of(2);
                let base_ptr = if is_bb2 { bb2_ptr } else { bb1_ptr };
                let offset = (i % DESCRIPTORS_PER_BUFFER) * CHUNK_SIZE;
                let len = core::cmp::min(CHUNK_SIZE, BOUNCE_SIZE - offset);

                let d = &mut (*desc_ptr)[i];
                d.buffer = base_ptr.add(offset);
                d.set_size(len);
                d.set_length(len);
                d.set_owner(Owner::Dma);

                // Link to next (Linear chain)
                d.next = if i < TOTAL_DESCRIPTORS - 1 {
                    addr_of_mut!((*desc_ptr)[i + 1]) as *mut _
                } else {
                    core::ptr::null_mut()
                };

                // EOF only at the very end of the 200-descriptor frame
                d.set_suc_eof(i == TOTAL_DESCRIPTORS - 1);
            }
            Self {
                descriptors: &mut (*desc_ptr),
                _buffer_area: core::slice::from_raw_parts_mut(bb1_ptr, BOUNCE_SIZE * 2),
            }
        }
    }
}

unsafe impl DmaTxBuffer for BounceBufferDma {
    type View = BounceBufferDma;
    type Final = BounceBufferDma;
    fn prepare(&mut self) -> Preparation {
        Preparation {
            start: &mut self.descriptors[0],
            accesses_psram: false,
            direction: TransferDirection::Out,
            burst_transfer: BurstConfig::default(),
            check_owner: Some(false),
            auto_write_back: false,
        }
    }
    fn into_view(self) -> Self::View {
        self
    }
    fn from_view(view: Self::View) -> Self {
        view
    }
}

/// Copies a specific chunk of pixel data from a source (PSRAM) to the
/// appropriate SRAM bounce buffer.
///
/// If chunk_num is 1, 3, 5 (Odd), we fill BOUNCE_BUF_2.
/// If chunk_num is 2, 4, 6 (Even), we fill BOUNCE_BUF_1.
///
/// # Safety
/// The caller must ensure that `source_ptr` points to a valid memory region
/// of at least `BOUNCE_SIZE` bytes.
#[ram]
pub fn fill_bounce_buffer(chunk_num: usize) {
    unsafe {
        let fb_ptr = FB1_ADDR.load(Ordering::Relaxed);
        let offset = chunk_num * 12 * SCREEN_WIDTH * BYTES_PER_PIXEL;
        let src_ptr: *const u8 = fb_ptr.add(offset);

        let dst_ptr = if !chunk_num.is_multiple_of(2) {
            addr_of_mut!(BOUNCE_BUF_2.0) as *mut u8
        } else {
            addr_of_mut!(BOUNCE_BUF_1.0) as *mut u8
        };

        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, BOUNCE_SIZE);
        rom_Cache_WriteBack_Addr(dst_ptr as u32, BOUNCE_SIZE as u32);
    }
}

unsafe extern "C" {
    // This tells the Rust compiler that the function is defined
    // elsewhere (the linker will find it in the ROM at 0x400016f8)
    pub fn rom_Cache_WriteBack_Addr(addr: u32, size: u32);
}
