fn main() {
    // Expose the cargo TARGET triple at compile time so runtime code
    // can locate dev-build sidecars (which keep the `-<triple>` suffix
    // until `tauri build` strips it for the production bundle).
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=TARGET_TRIPLE={target}");
    }
    tauri_build::build()
}
