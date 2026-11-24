#![deny(rust_2018_idioms)]

#[cfg(feature = "generate_binding")]
use std::path::PathBuf;
use std::{env, fmt::Display, path::Path};

/// Outputs the library-file's prefix as word usable for actual arguments on
/// commands or paths.
const fn rustc_linking_word(is_static_link: bool) -> &'static str {
    if is_static_link {
        "static"
    } else {
        "dylib"
    }
}

/// Generates a new binding at `src/lib.rs` using `src/wrapper.h`.
#[cfg(feature = "generate_binding")]
fn generate_binding() {
    const ALLOW_UNCONVENTIONALS: &'static str = "#![allow(non_upper_case_globals)]\n\
                                                 #![allow(non_camel_case_types)]\n\
                                                 #![allow(non_snake_case)]\n";

    let bindings = bindgen::Builder::default()
        .header("src/wrapper.h")
        .raw_line(ALLOW_UNCONVENTIONALS)
        .generate()
        .expect("Unable to generate binding");

    let binding_target_path = PathBuf::new().join("src").join("lib.rs");

    bindings
        .write_to_file(binding_target_path)
        .expect("Could not write binding to the file at `src/lib.rs`");

    println!("cargo:info=Successfully generated binding.");
}

fn build_opus(is_static: bool) {
    let opus_path = Path::new("opus");

    println!(
        "cargo:info=Opus source path used: {:?}.",
        opus_path
            .canonicalize()
            .expect("Could not canonicalise to absolute path")
    );

    println!("cargo:info=Building Opus via CMake.");
    let opus_build_dir = cmake::build(opus_path);
    link_opus(is_static, opus_build_dir.display())
}

fn link_opus(is_static: bool, opus_build_dir: impl Display) {
    let is_static_text = rustc_linking_word(is_static);

    println!(
        "cargo:info=Linking Opus as {} lib: {}",
        is_static_text, opus_build_dir
    );
    println!("cargo:rustc-link-lib={}=opus", is_static_text);
    println!("cargo:rustc-link-search=native={}/lib", opus_build_dir);
    println!("cargo:rustc-link-search=native={}/lib64", opus_build_dir);
}

#[cfg(any(unix, target_env = "gnu"))]
fn find_via_pkg_config(is_static: bool) -> bool {
    pkg_config::Config::new()
        .statik(is_static)
        .probe("opus")
        .is_ok()
}

/// Based on the OS or target environment we are building for,
/// this function will return an expected default library linking method.
///
/// If we build for Windows, MacOS, or Linux with musl, we will link statically.
/// However, if you build for Linux without musl, we will link dynamically.
///
/// **Info**:
/// This is a helper-function and may not be called if
/// if the `static`-feature is enabled, the environment variable
/// `LIBOPUS_STATIC` or `OPUS_STATIC` is set.
fn default_library_linking() -> bool {
    #[cfg(any(windows, target_os = "macos", target_env = "musl"))]
    {
        true
    }
    #[cfg(any(target_os = "freebsd", all(unix, target_env = "gnu")))]
    {
        false
    }
}

fn find_installed_opus() -> Option<String> {
    if let Ok(lib_directory) = env::var("LIBOPUS_LIB_DIR") {
        Some(lib_directory)
    } else if let Ok(lib_directory) = env::var("OPUS_LIB_DIR") {
        Some(lib_directory)
    } else {
        None
    }
}

fn is_static_build() -> bool {
    if cfg!(feature = "static") && cfg!(feature = "dynamic") {
        default_library_linking()
    } else if cfg!(feature = "static")
        || env::var("LIBOPUS_STATIC").is_ok()
        || env::var("OPUS_STATIC").is_ok()
    {
        println!("cargo:info=Static feature or environment variable found.");

        true
    } else if cfg!(feature = "dynamic") {
        println!("cargo:info=Dynamic feature enabled.");

        false
    } else {
        println!("cargo:info=No feature or environment variable found, linking by default.");

        default_library_linking()
    }
}

/// One source list from libopus's own make fragments (`NAME = \` then a path a line), so an Opus update brings its file list with it.
fn opus_sources(opus: &Path, fragment: &str, name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(opus.join(fragment)).expect("an Opus source list");
    let mut lines = text.lines().skip_while(|l| !l.starts_with(&format!("{name} =")));
    let mut out = Vec::new();
    let mut more = lines.next().is_some_and(|l| l.trim_end().ends_with('\\'));
    while more {
        let Some(l) = lines.next() else { break };
        let l = l.trim_end();
        more = l.ends_with('\\');
        let path = l.trim_end_matches('\\').trim();
        if !path.is_empty() {
            out.push(path.to_string());
        }
    }
    assert!(!out.is_empty(), "no sources under {} in {}", name, fragment);
    out
}

/// ferros (a JSON target, no CMake, no C library but the few calls its runtime exports: malloc, free and the mem functions): libopus built with the `cc` crate and the compiler and flags ferros's build exports (`CC_aarch64_unknown_ferros`, `CFLAGS_aarch64_unknown_ferros`).
/// Fixed point and no float API: ferros exports no C maths library, the fixed-point build calls none, and the float entry points would (their float-to-int conversion falls back to floor). Photon speaks i16 to Opus everywhere. The sysroot's stdlib.h declares no `abs`, so it is the compiler's. Arrays on the stack are C99 VLAs, not `alloca`.
fn build_opus_ferros() {
    let opus = Path::new("opus");
    let mut sources = opus_sources(opus, "opus_sources.mk", "OPUS_SOURCES");
    sources.extend(opus_sources(opus, "celt_sources.mk", "CELT_SOURCES"));
    sources.extend(opus_sources(opus, "silk_sources.mk", "SILK_SOURCES"));
    sources.extend(opus_sources(opus, "silk_sources.mk", "SILK_SOURCES_FIXED"));
    let mut build = cc::Build::new();
    for s in &sources {
        build.file(opus.join(s));
        println!("cargo:rerun-if-changed=opus/{s}");
    }
    build
        .include(opus.join("include"))
        .include(opus.join("celt"))
        .include(opus.join("silk"))
        .include(opus.join("silk/fixed"))
        .define("OPUS_BUILD", None)
        .define("FIXED_POINT", None)
        .define("DISABLE_FLOAT_API", None)
        .define("VAR_ARRAYS", None)
        .define("abs", "__builtin_abs")
        .warnings(false)
        .compile("opus");
    println!("cargo:info=Built Opus for ferros with cc: fixed point, {} sources.", sources.len());
}

fn main() {
    #[cfg(feature = "generate_binding")]
    generate_binding();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ferros") {
        build_opus_ferros();
        return;
    }

    let is_static = is_static_build();

    #[cfg(any(unix, target_env = "gnu"))]
    {
        if std::env::var("LIBOPUS_NO_PKG").is_ok() || std::env::var("OPUS_NO_PKG").is_ok() {
            println!("cargo:info=Bypassed `pkg-config`.");
        } else if find_via_pkg_config(is_static) {
            println!("cargo:info=Found `Opus` via `pkg_config`.");

            return;
        } else {
            println!("cargo:info=`pkg_config` could not find `Opus`.");
        }
    }

    if let Some(installed_opus) = find_installed_opus() {
        link_opus(is_static, installed_opus);
    } else {
        build_opus(is_static);
    }
}
