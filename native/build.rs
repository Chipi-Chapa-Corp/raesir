fn main() {
    println!("cargo:rerun-if-changed=../build/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../build/icon.ico")
            .set("ProductName", "Exposed Games")
            .set("FileDescription", "Exposed Games")
            .set("InternalName", "exposed-games")
            .set("OriginalFilename", "exposed-games.exe")
            .set("CompanyName", "Chipi Chapa Corp")
            .compile()
            .expect("compile Exposed Games Windows icon and version resources");
    }
}
