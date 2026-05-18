use std::process::Command;

fn main() {
    // Inject build date as an env var for version display
    let build_date = Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_DATE={}", build_date);

    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();

    // For cross-compilation via zigbuild, use locally extracted mingw-w64 windres/ar
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let mingw_bin = format!("{}/../mingw-bin", manifest_dir);
    let windres = format!("{}/x86_64-w64-mingw32-windres", mingw_bin);
    let ar = format!("{}/x86_64-w64-mingw32-ar", mingw_bin);

    if std::path::Path::new(&windres).exists() {
        res.set_windres_path(&windres);
        res.set_ar_path(&ar);
    }

    res.set_icon("icon.ico");
    res.set("CompanyName", "Digital Futures Consultancy LLP (Singapore)");
    res.set("FileDescription", "MKV Strip - Strip, extract, and add tracks in MKV files");
    res.set("ProductName", "mkv-strip");
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    res.compile().expect("Failed to compile Windows resources");
}