/**
 * @file wrapper.h
 * @brief Bindgen Entry Point & Environment Shim
 *
 * This file serves as the primary input for `bindgen` to generate Rust FFI bindings
 * for the LVGL library. 
 *
 * ### Why This File Is Necessary:
 * 1. **Standalone Environment:** Since this project is `no_std`, we lack the standard
 * system headers usually provided by a C compiler's environment (e.g., <stdint.h>).
 * This file manually defines fundamental types (uint32_t, size_t, etc.) to satisfy
 * Clang's parser during the binding generation process.
 * 2. **Library Aggregation:** It acts as a single include point for the entire LVGL
 * API, ensuring all necessary macros and structures are processed in the correct 
 * order.
 * 3. **Toolchain Compatibility:** It provides a shim for `va_list` and boolean types,
 * preventing "missing type" errors when the parser encounters LVGL's internal 
 * logging or event-handling functions.
 */
#ifndef WRAPPER_H
#define WRAPPER_H

// Manually define the basic types Clang is looking for
typedef unsigned char      uint8_t;
typedef unsigned short     uint16_t;
typedef unsigned int       uint32_t;
typedef unsigned long long uint64_t;
typedef signed char        int8_t;
typedef signed short       int16_t;
typedef signed int         int32_t;
typedef signed long long   int64_t;

typedef unsigned int       uintptr_t;
typedef signed int         intptr_t;

typedef unsigned int       size_t;
typedef int                bool;

typedef __builtin_va_list va_list;

#define true 1
#define false 0

// Now include the actual library
#include "lvgl/lvgl.h"

#endif