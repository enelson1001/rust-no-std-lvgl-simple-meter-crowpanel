use glob::glob;
use std::env;
use std::path::PathBuf;

fn main() {
    // --- 1. Your Existing Nice Linker Logic ---
    linker_be_nice();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");

    // --- LVGL Configuration & C-Compiling ---
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let c_code_dir = format!("{}/c_code", manifest_dir);

    // Tell the lvgl-rs crate where your lv_conf.h is
    println!("cargo:rustc-env=DEP_LV_CONFIG_PATH={}/", manifest_dir);

    let mut build = cc::Build::new();
    build
        .include("c_code/lvgl") // Finds lvgl.h
        .include("c_code/lvgl/src") // Finds internal headers
        .include("c_code") // Finds lv_conf.h
        .define("LV_CONF_INCLUDE_SIMPLE", None)
        .flag("-fno-common")
        .warnings(false);

    // Add custom fonts and images (if any)
    if let Ok(font_files) = std::fs::read_dir("c_code/assets") {
        for entry in font_files.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("c") {
                println!("cargo:rerun-if-changed={}", path.display());
                build.file(path);
            }
        }
    }

    // Automatically find all .c files in the lvgl directory
    for path in glob("c_code/lvgl/src/**/*.c")
        .expect("Failed to read glob pattern")
        .flatten()
    {
        let path_str = path.to_str().unwrap();
        // Optional: Skip demos/tests to save ESP32 flash space
        if !path_str.contains("demo") && !path_str.contains("test") {
            build.file(path);
        }
    }

    build.compile("lvgl_static");

    // Ensure we rebuild if headers or C code changes
    println!("cargo:rerun-if-changed=c_code/lv_conf.h");
    println!("cargo:rerun-if-changed=c_code/wrapper.h");
    println!("cargo:rerun-if-changed=c_code/lvgl/src");

    // Tell bindgen where the headers are
    let bindings = bindgen::Builder::default()
        .header("c_code/wrapper.h") // Use our new wrapper
        .use_core()
        .layout_tests(false)
        .ctypes_prefix("core::ffi")
        .clang_arg(format!("-I{}", c_code_dir)) // Point to c_code/
        .clang_arg(format!("-I{}/include_mock", c_code_dir)) // Point to c_code/include_mock/
        .clang_arg("-Ic_code/lvgl")
        .clang_arg("-Ic_code/lvgl/src")
        .clang_arg("--target=xtensa-esp32s3-none-elf")
        .clang_arg("-DLV_CONF_INCLUDE_SIMPLE")
        .clang_arg("-ffreestanding")
        .clang_arg("-D__builtin_va_list=void*")
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
