use super::*;
use std::{ffi::c_void, os::windows::ffi::OsStringExt, path::PathBuf, ptr};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

type Handle = *mut c_void;
type FileTime = [u32; 2];

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    fn WaitForSingleObject(object: Handle, wait_millis: u32) -> u32;
    fn GetProcessTimes(
        process: Handle,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        path: *mut u16,
        size: *mut u32,
    ) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, state: isize) -> i32;
    fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
    fn IsWindowVisible(window: Handle) -> i32;
    fn PostMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> i32;
}

struct Process(Handle);

impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

impl Process {
    fn inspect(pid: u32, session: u32) -> Option<(Self, ProcessId, PathBuf)> {
        if session_id(pid) != Some(session) {
            return None;
        }
        const QUERY_AND_SYNCHRONIZE: u32 = 0x00101000;
        const WAIT_TIMEOUT: u32 = 258;
        let handle = unsafe { OpenProcess(QUERY_AND_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let process = Self(handle);
        let (mut created, mut exited, mut kernel, mut user) = ([0; 2], [0; 2], [0; 2], [0; 2]);
        if unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT
            || unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
                == 0
        {
            return None;
        }
        let mut image = vec![0; 32768];
        let mut size = image.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, image.as_mut_ptr(), &mut size) } == 0 {
            return None;
        }
        let executable = PathBuf::from(std::ffi::OsString::from_wide(&image[..size as usize]));
        let started = u64::from(created[1]) << 32 | u64::from(created[0]);
        Some((process, ProcessId { pid, started }, normalized(&executable)))
    }
}

fn session_id(pid: u32) -> Option<u32> {
    let mut session = 0;
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

fn normalized(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().ok();
    let path = canonical
        .as_deref()
        .unwrap_or(path)
        .to_string_lossy()
        .to_lowercase();
    if let Some(network_path) = path.strip_prefix(r"\\?\unc\") {
        PathBuf::from(format!(r"\\{network_path}"))
    } else {
        PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path))
    }
}

pub(super) fn running(targets: &[Target<'_>]) -> Result<HashMap<String, Vec<ProcessId>>, String> {
    let session =
        session_id(unsafe { GetCurrentProcessId() }).ok_or("Could not read desktop session")?;
    let roots: Vec<_> = targets
        .iter()
        .map(|target| {
            (
                target.key,
                target
                    .roots
                    .iter()
                    .map(|root| normalized(root))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut running: HashMap<String, Vec<ProcessId>> = HashMap::new();
    for pid in system.processes().keys() {
        if let Some((_, id, executable)) = Process::inspect(pid.as_u32(), session) {
            for (key, roots) in &roots {
                if roots.iter().any(|root| executable.starts_with(root)) {
                    running.entry((*key).into()).or_default().push(id);
                }
            }
        }
    }
    for processes in running.values_mut() {
        processes.sort_by_key(|process| process.pid);
    }
    Ok(running)
}

pub(super) fn quit(target: &Target<'_>, processes: &[ProcessId]) -> Result<(), String> {
    let session =
        session_id(unsafe { GetCurrentProcessId() }).ok_or("Could not read desktop session")?;
    let roots: Vec<_> = target.roots.iter().map(|root| normalized(root)).collect();
    // Hold a handle to each verified process until the window messages have
    // been posted, so Windows cannot recycle a process ID during enumeration.
    let targets: Vec<_> = processes
        .iter()
        .filter_map(|id| {
            let (process, observed, executable) = Process::inspect(id.pid, session)?;
            (observed == *id && roots.iter().any(|root| executable.starts_with(root)))
                .then_some((process, id.pid))
        })
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    struct Windows<'a> {
        targets: &'a [(Process, u32)],
        sent: bool,
    }
    unsafe extern "system" fn close(window: Handle, state: isize) -> i32 {
        let state = unsafe { &mut *(state as *mut Windows<'_>) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if state.targets.iter().any(|(_, target)| *target == pid)
            && unsafe { IsWindowVisible(window) } != 0
        {
            state.sent |= unsafe { PostMessageW(window, 0x0010, 0, 0) } != 0; // WM_CLOSE
        }
        1
    }
    let mut state = Windows {
        targets: &targets,
        sent: false,
    };
    unsafe { EnumWindows(close, ptr::from_mut(&mut state) as isize) };
    if state.sent {
        Ok(())
    } else {
        Err("Could not close the game's window. Quit from inside the game.".into())
    }
}
