/// Desktop preview binary for the Android renderer pipeline.
/// Usage: cargo run -p epoca-android --bin epoca-android-preview -- path/to/app.zml

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let zml_path = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("examples/counter.zml");

    epoca_android::desktop_main(zml_path)
}
