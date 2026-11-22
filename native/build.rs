fn main() {
    println!("cargo:rerun-if-changed=../build/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../build/icon.ico")
            .set("ProductName", "Raesir")
            .set("FileDescription", "Raesir")
            .set("InternalName", "raesir")
            .set("OriginalFilename", "raesir.exe")
            .set("CompanyName", "Chipi Chapa Corp")
            .compile()
            .expect("compile Raesir Windows icon and version resources");
    }
}
