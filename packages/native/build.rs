extern crate napi_build;

fn main() {
    napi_build::setup();

    // `ui_thread`: platforms where GPUI runs on a `solo-ui` thread that the
    // renderer drives through `UiCommand`s (everything but macOS).
    println!("cargo::rustc-check-cfg=cfg(ui_thread)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if matches!(os.as_str(), "windows" | "linux" | "freebsd") {
        println!("cargo::rustc-cfg=ui_thread");
    }
}
