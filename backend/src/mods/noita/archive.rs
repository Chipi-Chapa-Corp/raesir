use super::*;

const MAX_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_FILES: usize = 50_000;

pub(super) fn destination(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || relative.to_string_lossy().contains(['\\', ':'])
        || relative
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err("Archive contains an unsafe path".into());
    }
    let path = root.join(relative);
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        if fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Refusing to install through a symbolic link".into());
        }
    }
    Ok(path)
}

pub(super) fn extract(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    if let Ok(mut archive) =
        zip::ZipArchive::new(fs::File::open(source).map_err(|e| e.to_string())?)
    {
        if archive.len() > MAX_FILES {
            return Err("Archive has too many files".into());
        }
        let mut bytes = 0_u64;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let path = destination(
                target,
                Path::new(
                    file.name()
                        .map_err(|e| e.to_string())?
                        .trim_end_matches('/'),
                ),
            )?;
            if file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err("Mod archives cannot contain symbolic links".into());
            }
            bytes = bytes
                .checked_add(file.size())
                .ok_or("Archive is too large")?;
            if bytes > MAX_BYTES {
                return Err("Expanded archive exceeds 1 GiB".into());
            }
            if file.is_dir() {
                fs::create_dir_all(path).map_err(|e| e.to_string())?;
                continue;
            }
            fs::create_dir_all(path.parent().ok_or("Invalid archive path")?)
                .map_err(|e| e.to_string())?;
            let mut output = fs::File::create(path).map_err(|e| e.to_string())?;
            let size = file.size();
            let written =
                std::io::copy(&mut file.by_ref().take(size.saturating_add(1)), &mut output)
                    .map_err(|e| e.to_string())?;
            if written != file.size() {
                return Err("Archive entry has an invalid size".into());
            }
        }
        return Ok(());
    }
    // ModWorkshop also hosts 7z/RAR archives. Libarchive streams entries through
    // the same checked paths; it never extracts files itself or runs programs.
    unsafe { extract_libarchive(source, target) }
}

unsafe fn extract_libarchive(source: &Path, target: &Path) -> Result<(), String> {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    unsafe {
        let library = libloading::Library::new(if cfg!(target_os = "windows") {
            "archive.dll"
        } else {
            "libarchive.so.13"
        })
        .map_err(|_| {
            "This archive needs libarchive (ZIP files are supported directly)".to_owned()
        })?;
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {
                *library
                    .get::<$ty>(concat!($name, "\0").as_bytes())
                    .map_err(|e| e.to_string())?
            };
        }
        let new = symbol!("archive_read_new", unsafe extern "C" fn() -> *mut c_void);
        let free = symbol!(
            "archive_read_free",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        let formats = symbol!(
            "archive_read_support_format_all",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        let filters = symbol!(
            "archive_read_support_filter_all",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        let open = symbol!(
            "archive_read_open_filename",
            unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> c_int
        );
        let next = symbol!(
            "archive_read_next_header",
            unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> c_int
        );
        let pathname = symbol!(
            "archive_entry_pathname",
            unsafe extern "C" fn(*mut c_void) -> *const c_char
        );
        let kind = symbol!(
            "archive_entry_filetype",
            unsafe extern "C" fn(*mut c_void) -> u32
        );
        let hardlink = symbol!(
            "archive_entry_hardlink",
            unsafe extern "C" fn(*mut c_void) -> *const c_char
        );
        let read = symbol!(
            "archive_read_data",
            unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> isize
        );
        let error = symbol!(
            "archive_error_string",
            unsafe extern "C" fn(*mut c_void) -> *const c_char
        );
        let archive = new();
        if archive.is_null() {
            return Err("Could not allocate archive reader".into());
        }
        struct Reader {
            raw: *mut c_void,
            free: unsafe extern "C" fn(*mut c_void) -> c_int,
        }
        impl Drop for Reader {
            fn drop(&mut self) {
                unsafe {
                    (self.free)(self.raw);
                }
            }
        }
        let _reader = Reader { raw: archive, free };
        formats(archive);
        filters(archive);
        let filename =
            CString::new(source.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        if open(archive, filename.as_ptr(), 64 * 1024) < 0 {
            return Err("Could not open the mod archive".into());
        }
        let mut total = 0_u64;
        let mut files = 0;
        loop {
            let mut entry = std::ptr::null_mut();
            let result = next(archive, &mut entry);
            if result == 1 {
                break;
            }
            if result < 0 {
                let message = error(archive);
                return Err(if message.is_null() {
                    "Invalid archive".into()
                } else {
                    CStr::from_ptr(message).to_string_lossy().into_owned()
                });
            }
            files += 1;
            if files > MAX_FILES {
                return Err("Archive has too many files".into());
            }
            let name = pathname(entry);
            if name.is_null() {
                return Err("Archive entry is unnamed".into());
            }
            let name = CStr::from_ptr(name)
                .to_str()
                .map_err(|_| "Archive filenames must be UTF-8")?;
            let path = destination(target, Path::new(name.trim_end_matches('/')))?;
            let filetype = kind(entry);
            if !hardlink(entry).is_null() || !matches!(filetype, 0o100000 | 0o040000) {
                return Err("Archive contains links or special files".into());
            }
            if filetype == 0o040000 {
                fs::create_dir_all(path).map_err(|e| e.to_string())?;
                continue;
            }
            fs::create_dir_all(path.parent().ok_or("Invalid path")?).map_err(|e| e.to_string())?;
            let mut output = fs::File::create(path).map_err(|e| e.to_string())?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = read(archive, buffer.as_mut_ptr().cast(), buffer.len());
                if count == 0 {
                    break;
                }
                if count < 0 {
                    return Err("Could not decode archive entry".into());
                }
                total += count as u64;
                if total > MAX_BYTES {
                    return Err("Expanded archive exceeds 1 GiB".into());
                }
                output
                    .write_all(&buffer[..count as usize])
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}
