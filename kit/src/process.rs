//! Starting and finding processes (DESIGN.md 5): scripts in named job objects, programs without a console
//! window, "open this file or address", waiting for an exit. Direct API calls – no hidden PowerShell.

use std::io;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::SystemServices::{JOB_OBJECT_ASSIGN_PROCESS, JOB_OBJECT_QUERY, JOB_OBJECT_TERMINATE};
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::ShellExecuteW;

use crate::ipc::{sandbox, wide};

/// The job a script of the program `id` runs in.
pub fn job_name(id: &str) -> String {
    format!("WindowsToolbox.{id}{}", sandbox())
}

/// A named Windows job object (DESIGN.md 4.2): "running" = the job has processes, Stop = end all of them –
/// the script together with everything it started.
pub struct Job(HANDLE);
unsafe impl Send for Job {}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

impl Job {
    /// Opens the job if it exists (somebody holds a handle to it), else `None`.
    pub fn open(id: &str) -> Option<Job> {
        let name = wide(&job_name(id));
        let h = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE | JOB_OBJECT_ASSIGN_PROCESS, 0, name.as_ptr()) };
        if h.is_null() { None } else { Some(Job(h)) }
    }

    /// Creates the job, or opens it if it exists already.
    pub fn create(id: &str) -> io::Result<Job> {
        let name = wide(&job_name(id));
        let h = unsafe { CreateJobObjectW(null(), name.as_ptr()) };
        if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Job(h)) }
    }

    pub fn active_processes(&self) -> u32 {
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            QueryInformationJobObject(self.0, JobObjectBasicAccountingInformation, &mut info as *mut _ as *mut _, size_of_val(&info) as u32, null_mut())
        };
        if ok == 0 { 0 } else { info.ActiveProcesses }
    }

    /// Ends every process in the job.
    pub fn terminate(&self) -> bool {
        unsafe { TerminateJobObject(self.0, 1) != 0 }
    }

    /// Starts `cmdline` without a window and suspended, puts it into the job, then lets it run – so even a child
    /// it starts in its first instant belongs to the job. Returns the process id.
    pub fn spawn(&self, cmdline: &str, cwd: Option<&Path>) -> io::Result<u32> {
        let mut cmd = wide(cmdline);
        let cwd = cwd.map(|p| wide(&p.to_string_lossy()));
        // The job's name lives only as long as somebody holds a handle to the job – processes inside it do not
        // count. So the script inherits one handle (and only that one, through the handle list): the job stays
        // findable by name as long as the script's first process runs, even after the agent was restarted.
        let mut inherit: HANDLE = null_mut();
        unsafe {
            let me = GetCurrentProcess();
            if DuplicateHandle(me, self.0, me, &mut inherit, 0, 1, DUPLICATE_SAME_ACCESS) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let mut size = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size) };
        let mut attr_buf = vec![0u8; size];
        let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESHOWWINDOW;
        si.StartupInfo.wShowWindow = 0; // SW_HIDE
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let initialised = unsafe { InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) != 0 };
        let ok = unsafe {
            initialised
                && UpdateProcThreadAttribute(
                    attrs,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    &inherit as *const HANDLE as *const _,
                    size_of::<HANDLE>(),
                    null_mut(),
                    null(),
                ) != 0
                && CreateProcessW(
                    null(),
                    cmd.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                    null(),
                    cwd.as_ref().map_or(null(), |c| c.as_ptr()),
                    &si.StartupInfo,
                    &mut pi,
                ) != 0
        };
        let err = io::Error::last_os_error();
        unsafe {
            if initialised {
                DeleteProcThreadAttributeList(attrs);
            }
            CloseHandle(inherit);
        }
        if !ok {
            return Err(err);
        }
        unsafe {
            if AssignProcessToJobObject(self.0, pi.hProcess) == 0 {
                let e = io::Error::last_os_error();
                TerminateProcess(pi.hProcess, 1);
                CloseHandle(pi.hThread);
                CloseHandle(pi.hProcess);
                return Err(e);
            }
            ResumeThread(pi.hThread);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        }
        Ok(pi.dwProcessId)
    }
}

/// One argument, quoted the way Windows programs take their command line apart.
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
            continue;
        }
        // Backslashes only count as escapes in front of a quote – there they are doubled.
        out.push_str(&"\\".repeat(if c == '"' { backslashes * 2 + 1 } else { backslashes }));
        backslashes = 0;
        out.push(c);
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Starts a program in its own folder, without a console window. Returns the process id.
pub fn spawn(exe: &Path, args: &[&str]) -> io::Result<u32> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).creation_flags(CREATE_NO_WINDOW);
    if let Some(dir) = exe.parent().filter(|d| !d.as_os_str().is_empty()) {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().map(|child| child.id())
}

fn shell_execute(verb: &str, target: &str, dir: Option<&Path>) -> bool {
    let (verb, target) = (wide(verb), wide(target));
    let dir = dir.map(|d| wide(&d.to_string_lossy()));
    // More than 32 means success – a leftover of 16-bit Windows.
    let r = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), target.as_ptr(), null(), dir.as_ref().map_or(null(), |d| d.as_ptr()), 1) };
    r as usize > 32
}

/// Starts a scheduled task (`setup::task_name`) – the way a program without admin rights starts something with
/// them, without a prompt (DESIGN.md 10). `true` if Task Scheduler took it.
pub fn run_task(name: &str) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("schtasks.exe")
        .args(["/run", "/tn", name])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Opens a file, a folder or an address (`https://…`, `uplay://…`, `ms-settings:…`) with whatever Windows uses
/// for it – like a double-click.
pub fn open(target: &str) -> bool {
    shell_execute("open", target, None)
}

/// Puts a text into the clipboard – what a "Copy" button of a page does.
pub fn copy_text(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    let data = wide(text);
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
        let mut ok = false;
        if !memory.is_null() {
            let target = GlobalLock(memory) as *mut u16;
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
                GlobalUnlock(memory);
                // From here on the memory belongs to Windows.
                ok = !SetClipboardData(CF_UNICODETEXT, memory as HANDLE).is_null();
            }
            if !ok {
                GlobalFree(memory);
            }
        }
        CloseClipboard();
        ok
    }
}

/// Waits until the process has ended – at most `timeout`. `true` = ended (or never existed). Works for processes
/// with admin rights too: waiting needs no more than the right to wait.
pub fn wait_exit(pid: u32, timeout: Duration) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return GetLastError() == ERROR_INVALID_PARAMETER;
        }
        let r = WaitForSingleObject(h, timeout.as_millis().min(u32::MAX as u128 - 1) as u32);
        CloseHandle(h);
        r == WAIT_OBJECT_0
    }
}

/// The ids of all processes that run this exe file (compared by full path, upper and lower case ignored).
pub fn running(exe: &Path) -> Vec<u32> {
    let want = normalise(exe);
    let Some(file) = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()) else { return vec![] };
    let mut pids = vec![];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return pids;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            // Cheap first: the file name. Only for a match the full path is asked for.
            if String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase() == file && image_path(entry.th32ProcessID).is_some_and(|p| p == want) {
                pids.push(entry.th32ProcessID);
            }
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    pids
}

fn normalise(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    full.to_string_lossy().trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase()
}

fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]).to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(r"C:\My Folder\a.ps1"), r#""C:\My Folder\a.ps1""#);
        assert_eq!(quote(r"C:\My Folder\"), r#""C:\My Folder\\""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(""), r#""""#);
    }

    #[test]
    fn this_test_runs() {
        let me = std::env::current_exe().unwrap();
        assert!(running(&me).contains(&std::process::id()));
        assert!(running(Path::new(r"C:\nowhere\nothing.exe")).is_empty());
        assert!(wait_exit(0xffff_fff0, Duration::from_millis(10)), "a process that does not exist has ended");
        assert!(!wait_exit(std::process::id(), Duration::from_millis(10)));
    }

    /// DESIGN.md 4.2: a script in its job is found by name – also by somebody who did not start it – and is
    /// stopped as a whole.
    #[test]
    fn a_script_in_its_job() {
        let id = format!("kit-test-{}", std::process::id());
        assert!(Job::open(&id).is_none());
        let job = Job::create(&id).unwrap();
        // The script starts a child of its own; both must end with the job.
        let script = "Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep 60'; Start-Sleep 60";
        let cmd = format!("powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -Command {}", quote(script));
        job.spawn(&cmd, None).unwrap();
        // Our own handle is closed: now only the script itself keeps the name alive.
        drop(job);
        let t = std::time::Instant::now();
        let found = loop {
            let job = Job::open(&id).expect("the job is still found by its name");
            if job.active_processes() >= 2 {
                break job;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "the script's child never appeared");
            std::thread::sleep(Duration::from_millis(100));
        };
        assert!(found.terminate());
        let t = std::time::Instant::now();
        while found.active_processes() > 0 {
            assert!(t.elapsed() < Duration::from_secs(5), "the job did not end");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(found);
        // Windows needs a moment to take an ended process apart – with it goes the handle that kept the name.
        let t = std::time::Instant::now();
        while Job::open(&id).is_some() {
            assert!(t.elapsed() < Duration::from_secs(5), "with its last process the job's name must be gone");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
