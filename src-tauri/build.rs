fn main() {
    // Core tests intentionally omit the desktop feature, so they must not
    // generate or link Tauri's platform shell artifacts.
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        tauri_build::build();
    }
}
