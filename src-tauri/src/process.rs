//! Child-process helpers shared by everything that runs a managed binary.

use std::ffi::OsStr;
use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::process::Command;

/// `CREATE_NO_WINDOW`: without it every yt-dlp/ffmpeg launch flashes a console.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A command with no stdin and no console window.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Runs a command to completion and captures its output. Returns `None` if
/// it outlives `limit`, in which case its whole process tree is killed.
pub async fn output_within(mut cmd: Command, limit: Duration) -> std::io::Result<Option<Output>> {
    let child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let pid = child.id();
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(output) => output.map(Some),
        Err(_) => {
            if let Some(pid) = pid {
                kill_tree(pid).await;
            }
            Ok(None)
        }
    }
}

/// `CREATE_NEW_PROCESS_GROUP`: the process heads a group of its own, which
/// Ctrl+Break can be sent to (see [`interrupt_tree`]).
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Runs `cmd` as the head of a process group of its own, so it can be asked
/// to stop with Ctrl+Break instead of being killed (see [`interrupt_tree`]).
pub fn in_own_group(cmd: &mut Command) {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}

/// The argument that makes Siphon's own executable a helper which sends
/// Ctrl+Break to a process group and exits (see [`send_break`]).
pub const INTERRUPT_ARG: &str = "--siphon-interrupt";

/// The process group a helper run was asked to interrupt: `siphon.exe
/// --siphon-interrupt <pid>`. None for an ordinary launch.
pub fn interrupt_request() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    (args.next()? == INTERRUPT_ARG).then(|| args.next()?.parse().ok())?
}

/// Sends Ctrl+Break to the process group `pid` heads. Only a process on the
/// same console can, which is why a helper process does it (see
/// [`interrupt_tree`]): it leaves its own console, joins the group's, and
/// ignores the break itself.
pub fn send_break(pid: u32) -> bool {
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler, CTRL_BREAK_EVENT,
    };
    // SAFETY: plain Win32 calls with no pointers; the handler routine is null,
    // which means "ignore Ctrl+C and Ctrl+Break in this process".
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return false;
        }
        SetConsoleCtrlHandler(None, 1);
        GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) != 0
    }
}

/// Asks the process group `pid` heads to stop, the way a person pressing
/// Ctrl+Break would, and waits until it and everything it started have
/// ended. ffmpeg takes it as "finish the file and quit"; yt-dlp ends at once,
/// but the ffmpeg it started goes on writing for a moment, so the wait is for
/// the whole tree, as it was when asked. Whatever is still running after
/// `grace` is killed.
///
/// `helper` is Siphon's own executable (see [`INTERRUPT_ARG`]).
pub async fn interrupt_tree(helper: &std::path::Path, pid: u32, grace: Duration) {
    let mut tree = descendants(pid);
    tree.push(pid);
    let mut cmd = command(helper);
    cmd.arg(INTERRUPT_ARG).arg(pid.to_string());
    let sent = matches!(output_within(cmd, Duration::from_secs(10)).await, Ok(Some(output)) if output.status.success());
    let deadline = tokio::time::Instant::now() + grace;
    if sent {
        while tree.iter().any(|pid| alive(*pid)) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    // A process that is already gone can't be killed; the others go now.
    for pid in tree.into_iter().filter(|pid| alive(*pid)) {
        let mut killer = command("taskkill");
        killer.args(["/PID", &pid.to_string(), "/F"]);
        let _ = killer.stdout(Stdio::null()).stderr(Stdio::null()).status().await;
    }
}

/// Every process `pid` started, and everything they started, as they are now.
/// Read once, before anything is stopped: a process whose parent has ended
/// can't be traced back to it any more.
pub fn descendants(pid: u32) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut parents: Vec<(u32, u32)> = Vec::new();
    // SAFETY: the snapshot handle is checked and closed; the entry is sized as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    children_of(pid, &parents)
}

/// The processes below `root` in a list of (process, parent) pairs.
fn children_of(root: u32, parents: &[(u32, u32)]) -> Vec<u32> {
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, of) in parents {
            // A process can't be its own child (the idle process lists itself that way).
            if of == parent && pid != parent && pid != root && !found.contains(&pid) {
                found.push(pid);
                frontier.push(pid);
            }
        }
    }
    found
}

/// Whether the process `pid` is still running.
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is checked and closed; the exit code is written to a local.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        running
    }
}

/// Makes `child`, and everything it starts, end when Siphon does, however
/// Siphon ends. Without it, a crash (or Task Manager) leaves yt-dlp and its
/// ffmpeg running: a live recording would go on growing on disk with nobody to
/// stop it, under a file Siphon tries to finish when it starts again.
///
/// One Windows job object for the whole app, set to end its processes once
/// its last handle closes, which is when Siphon's process goes.
pub fn end_with_siphon(child: &tokio::process::Child) {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // The handle as a number: a raw pointer isn't `Sync`. Zero if it couldn't be made.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: a fresh unnamed job and a zeroed limit structure of the size the call is told.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set == 0 {
                return 0;
            }
            // Never closed: it has to last as long as Siphon does.
            job as usize
        }
    });
    if let (true, Some(handle)) = (job != 0, child.raw_handle()) {
        // SAFETY: both handles are valid; a process that already ended just isn't added.
        unsafe {
            AssignProcessToJobObject(job as _, handle as _);
        }
    }
}

/// Kills a process and all of its descendants.
///
/// yt-dlp's Windows build is a self-extracting launcher that spawns the real
/// interpreter, which in turn spawns ffmpeg. Killing only the launcher would
/// leave the others running, so the whole tree has to go.
pub async fn kill_tree(pid: u32) {
    let mut killer = command("taskkill");
    killer.args(["/PID", &pid.to_string(), "/T", "/F"]);
    let _ = killer.stdout(Stdio::null()).stderr(Stdio::null()).status().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_process_below_one() {
        // 10 started 11 and 12; 11 started 13; 20 is someone else's; 0 lists itself.
        let parents = [(11, 10), (12, 10), (13, 11), (20, 1), (0, 0), (10, 4)];
        let mut below = children_of(10, &parents);
        below.sort_unstable();
        assert_eq!(below, [11, 12, 13]);
        assert!(children_of(0, &parents).is_empty());
    }

    #[tokio::test]
    async fn interrupting_ends_the_whole_tree_even_when_the_helper_cant_run() {
        let mut cmd = command("cmd");
        cmd.args(["/c", "ping -n 60 127.0.0.1 > NUL"]);
        in_own_group(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id().unwrap();
        let mut below = Vec::new();
        for _ in 0..50 {
            below = descendants(pid);
            if !below.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!below.is_empty(), "ping never started under cmd");

        let started = std::time::Instant::now();
        interrupt_tree(
            std::path::Path::new(r"C:\no\such\helper.exe"),
            pid,
            Duration::from_secs(30),
        )
        .await;
        // Without a helper there is nothing to wait for: it goes straight to killing.
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = child.wait().await;
        assert!(!alive(pid));
        assert!(below.iter().all(|pid| !alive(*pid)), "a process below was left running");
    }
}
