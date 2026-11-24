//! Build script for serial_monitor.dll
//!
//! Copies serial_monitor_hook.dll (pre-built) into OUT_DIR so lib.rs can
//! embed it with `include_bytes!(concat!(env!("OUT_DIR"), "/serial_monitor_hook.dll"))`.
//!
//! Build serial_monitor_hook FIRST, then serial_monitor.
//! Do NOT call `cargo` from this build script -- that deadlocks because
//! cargo holds the build-directory file lock.
//!
//! Recommended workflow: run `.\build.ps1` from serial_monitor_rs/.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
    );
    // serial-pro-rs 工作区布局：crates/serial_monitor -> 工作区根在两级之上
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("serial_monitor crate is not in crates/ layout");

    let target = std::env::var("TARGET").unwrap_or_else(|_| "x86_64-pc-windows-msvc".into());
    let _profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".into());

    // 兼容多种 hook dll 位置：
    // 1. 工作区 target（同工具链构建）
    // 2. hook crate 独立 target（serial_monitor_hook 不在主 workspace 内，
    //    需 nightly：cargo +nightly build --manifest-path crates/serial_monitor_hook/Cargo.toml）
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in [workspace_root, manifest_dir.as_path()] {
        for profile in ["debug", "release"] {
            candidates.push(
                root.join("target")
                    .join(profile)
                    .join("serial_monitor_hook.dll"),
            );
            candidates.push(
                root.join("target")
                    .join(&target)
                    .join(profile)
                    .join("serial_monitor_hook.dll"),
            );
        }
    }
    let hook_dll_src = match candidates.iter().find(|p| p.exists()) {
        Some(p) => p.clone(),
        None => {
            let list = candidates
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n");
            panic!(
                "\n\nserial_monitor_hook.dll not found.\nTried:\n{list}\n\n\
                 Build the hook DLL first (requires nightly toolchain):\n\
                 \n  cargo +nightly build --release --manifest-path crates/serial_monitor_hook/Cargo.toml\n"
            );
        }
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let dst = out_dir.join("serial_monitor_hook.dll");
    std::fs::copy(&hook_dll_src, &dst)
        .expect("Failed to copy serial_monitor_hook.dll to OUT_DIR");

    // Re-run when the hook DLL or its source changes.
    println!("cargo:rerun-if-changed={}", hook_dll_src.display());
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root
            .join("crates")
            .join("serial_monitor_hook")
            .join("src")
            .join("lib.rs")
            .display()
    );
}
