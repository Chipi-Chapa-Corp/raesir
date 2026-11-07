//! Real executable identity for process tests, independent of coreutils wrappers.
use std::{
    path::Path,
    process::{Child, Command},
    sync::LazyLock,
};

pub fn spawn(executable: &Path) -> Child {
    static FIXTURE: LazyLock<tempfile::TempDir> = LazyLock::new(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process.c");
        let status = Command::new("cc")
            .arg(source)
            .arg("-o")
            .arg(dir.path().join("process"))
            .status()
            .unwrap();
        assert!(status.success(), "compile the process identity fixture");
        dir
    });
    // Linking publishes the already-closed executable without a writable file
    // descriptor that another test's concurrent fork could inherit (ETXTBSY).
    std::fs::hard_link(FIXTURE.path().join("process"), executable).unwrap();
    Command::new(executable).spawn().unwrap()
}
