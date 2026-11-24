//! What a program needs from Windows (DESIGN.md 9): a driver from winget, a download, a file in the protected
//! folder, a scheduled task – declared as steps, checked without admin rights, done with **one** admin prompt and
//! undone again by Uninstall.
//!
//! The protected folder `C:\Program Files\WindowsToolbox\<id>\` is the only place where something lies that later
//! runs with admin rights without asking (DESIGN.md 10): only admins can write there, so no program of the user can
//! swap the file a task starts. The task is `\WindowsToolbox\<id>` – it has no trigger and runs only when the
//! program starts it (`process::run_task`).
//!
//! How it runs: what needs no admin rights (the downloads) happens in the program itself, into a temporary folder,
//! checked by their SHA-256. Then the program starts its own exe once more with admin rights – Windows asks – with
//! the internal argument `--kit-setup <job file>` (`app::init` takes it). That short-lived helper does the job's list
//! and nothing else: it writes only below the protected folder, registers only tasks below `\WindowsToolbox\`, checks
//! every download's checksum again on the bytes it unpacks, and writes what happened next to the job file.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ipc::{sandbox, wide};
use crate::update::zip;

/// Internal argument between our own processes: run a setup job with admin rights.
pub const HELPER: &str = "--kit-setup";
/// The largest download a step takes.
const DOWNLOAD_LIMIT: usize = 64 << 20;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Next to a download's folder: the checksum of what was unpacked there.
const STAMP: &str = ".sha256";

/// One thing a program needs from Windows.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A package from winget (a driver). Done when the Windows service `service` exists.
    Winget { id: String, name: String, service: String },
    /// A zip from the internet in a pinned version, checked by SHA-256 and unpacked into `into` – a folder in the
    /// protected folder.
    Download { name: String, version: String, url: String, sha256: String, into: String },
    /// A file next to the program's exe, copied into `into` in the protected folder (`""` = the folder itself).
    /// Done when the copy there has the same bytes.
    Copy { file: String, into: String },
    /// The task `\WindowsToolbox\<id>` that starts `run` (a path in the protected folder) with admin rights.
    Task { run: String },
}

impl Step {
    /// What the page shows for this step.
    pub fn name(&self) -> String {
        match self {
            Step::Winget { name, .. } => name.clone(),
            Step::Download { name, version, .. } => format!("{name} {version}"),
            Step::Copy { file, .. } => file.clone(),
            Step::Task { .. } => crate::t!("kit.setup.task"),
        }
    }
}

/// How far a step is.
#[derive(Debug, Clone, PartialEq)]
pub enum Done {
    Yes,
    No,
    /// There, but another version or other bytes – "Set up" replaces it.
    Outdated,
}

// ---------- where things lie ----------

/// Internal, for tests: another folder instead of `C:\Program Files\WindowsToolbox`.
pub const ROOT_VARIABLE: &str = "TOOLBOX_PROTECTED_ROOT";

/// `C:\Program Files\WindowsToolbox` – the folder all protected folders lie in.
pub fn protected_root() -> PathBuf {
    if let Some(dir) = std::env::var_os(ROOT_VARIABLE) {
        return PathBuf::from(dir);
    }
    let program_files = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).unwrap_or_else(|| "C:\\Program Files".into());
    PathBuf::from(program_files).join("WindowsToolbox")
}

/// `C:\Program Files\WindowsToolbox\<id>`.
pub fn protected_dir(id: &str) -> PathBuf {
    protected_root().join(format!("{id}{}", sandbox()))
}

/// `\WindowsToolbox\<id>` – the program's task.
pub fn task_name(id: &str) -> String {
    format!("\\WindowsToolbox\\{id}{}", sandbox())
}

fn place(id: &str, into: &str) -> PathBuf {
    let dir = protected_dir(id);
    if into.is_empty() { dir } else { dir.join(into) }
}

fn hidden(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null());
    cmd
}

// ---------- checking (no admin rights) ----------

/// How far `step` of the program `id` is.
pub fn done(id: &str, step: &Step) -> Done {
    match step {
        Step::Winget { service, .. } => yes(service_exists(service)),
        Step::Download { sha256, into, .. } => match std::fs::read_to_string(place(id, into).join(STAMP)) {
            Ok(stamp) if stamp.trim().eq_ignore_ascii_case(sha256) => Done::Yes,
            Ok(_) => Done::Outdated,
            Err(_) => Done::No,
        },
        Step::Copy { file, into } => {
            let target = place(id, into).join(file);
            match (std::fs::read(crate::app::program_dir().join(file)), std::fs::read(&target)) {
                (Ok(a), Ok(b)) if a == b => Done::Yes,
                (_, Ok(_)) => Done::Outdated,
                _ => Done::No,
            }
        }
        Step::Task { run } => match task_command(&task_name(id)) {
            Some((command, highest)) if same(&command, &protected_dir(id).join(run)) && highest => Done::Yes,
            Some(_) => Done::Outdated,
            None => Done::No,
        },
    }
}

fn yes(b: bool) -> Done {
    if b { Done::Yes } else { Done::No }
}

fn same(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().trim_matches('"').eq_ignore_ascii_case(&b.to_string_lossy())
}

/// The steps that are not done yet.
pub fn pending<'a>(id: &str, steps: &'a [Step]) -> Vec<&'a Step> {
    steps.iter().filter(|s| done(id, s) != Done::Yes).collect()
}

/// Whether anything of the program's setup is in Windows: its task or its protected folder.
pub fn present(id: &str) -> bool {
    protected_dir(id).exists() || task_command(&task_name(id)).is_some()
}

/// Whether a Windows service of this name exists (asked with the least rights there are).
pub fn service_exists(name: &str) -> bool {
    use windows_sys::Win32::System::Services::*;
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return false;
        }
        let name = wide(name);
        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        let found = !service.is_null();
        if found {
            CloseServiceHandle(service);
        }
        CloseServiceHandle(manager);
        found
    }
}

/// The task's program and whether it runs with admin rights – `None` if there is no such task.
fn task_command(name: &str) -> Option<(PathBuf, bool)> {
    let out = hidden("schtasks.exe").args(["/query", "/tn", name, "/xml", "ONE"]).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    // schtasks writes the XML in the console's code page; the parts we read are plain ASCII or a path.
    let xml = String::from_utf8_lossy(&out.stdout);
    let command = between(&xml, "<Command>", "</Command>")?;
    Some((PathBuf::from(unescape(command)), xml.contains("<RunLevel>HighestAvailable</RunLevel>")))
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    Some(&text[from..from + text[from..].find(end)?])
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// The ids whose task or protected folder is in Windows (for Clean up and Uninstall all). Only this sandbox's.
pub fn installed() -> Vec<String> {
    let suffix = sandbox();
    // A sandbox's names end in its suffix; ids themselves have no dot.
    let ours = |name: &str| -> Option<String> {
        let id = if suffix.is_empty() { name } else { name.strip_suffix(suffix)? };
        (!id.is_empty() && !id.contains('.')).then(|| id.to_string())
    };
    let mut ids: Vec<String> = vec![];
    if let Ok(entries) = std::fs::read_dir(protected_root()) {
        for e in entries.flatten().filter(|e| e.path().is_dir()) {
            if let Some(id) = ours(&e.file_name().to_string_lossy()) {
                ids.push(id);
            }
        }
    }
    if let Ok(out) = hidden("schtasks.exe").args(["/query", "/fo", "csv", "/nh"]).stderr(Stdio::null()).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let name = line.split(',').next().unwrap_or_default().trim_matches('"');
            if let Some(id) = name.strip_prefix("\\WindowsToolbox\\").and_then(ours) {
                ids.push(id);
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

// ---------- doing it (one admin prompt) ----------

/// What the helper with admin rights is asked to do – one entry per action.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "do", rename_all = "snake_case")]
enum Action {
    WingetInstall { id: String, service: String },
    WingetUninstall { id: String, service: String },
    /// `zip`: the downloaded file in the job's folder.
    Unpack { zip: PathBuf, sha256: String, into: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
    /// `user`: the SID of the user the program runs as – the task runs as them (with their admin rights).
    Task { name: String, run: PathBuf, user: String, description: String },
    RemoveTask { name: String },
    RemoveDir { dir: PathBuf },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct Job {
    actions: Vec<Action>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct Outcome {
    ok: bool,
    message: String,
}

/// "Set up": does every step that is not done. Downloads first (no admin rights), then one admin prompt for the
/// rest. `description`: what Task Scheduler shows for the task. An error is a text for the page.
pub fn run(id: &str, steps: &[Step], description: &str) -> Result<(), String> {
    let pending = pending(id, steps);
    if pending.is_empty() {
        return Ok(());
    }
    let work = JobFolder::new(id)?;
    let mut actions = vec![];
    for step in pending {
        actions.push(match step {
            Step::Winget { id: package, service, .. } => Action::WingetInstall { id: package.clone(), service: service.clone() },
            Step::Download { name, url, sha256, into, .. } => {
                let data = crate::http::get(url, &[], DOWNLOAD_LIMIT).map_err(|e| crate::t!("kit.setup.download_failed", name = name, why = e))?;
                if !data.ok() {
                    return Err(crate::t!("kit.setup.download_failed", name = name, why = format!("HTTP {}", data.status)));
                }
                if !hex(&Sha256::digest(&data.body)).eq_ignore_ascii_case(sha256) {
                    return Err(crate::t!("kit.setup.checksum", name = name));
                }
                let file = work.dir.join(format!("{}.zip", actions.len()));
                std::fs::write(&file, &data.body).map_err(|e| e.to_string())?;
                Action::Unpack { zip: file, sha256: sha256.clone(), into: place(id, into) }
            }
            Step::Copy { file, into } => Action::Copy { from: crate::app::program_dir().join(file), to: place(id, into).join(file) },
            Step::Task { run } => Action::Task { name: task_name(id), run: protected_dir(id).join(run), user: user_sid().unwrap_or_default(), description: description.to_string() },
        });
    }
    work.run(actions)
}

/// "Uninstall": the task and the protected folder go; the winget packages only with `with_winget` (other programs
/// may use a driver too). Nothing there → no prompt.
pub fn remove(id: &str, steps: &[Step], with_winget: bool) -> Result<(), String> {
    let mut actions = vec![];
    if task_command(&task_name(id)).is_some() {
        actions.push(Action::RemoveTask { name: task_name(id) });
    }
    if protected_dir(id).exists() {
        actions.push(Action::RemoveDir { dir: protected_dir(id) });
    }
    if with_winget {
        for step in steps {
            if let Step::Winget { id: package, service, .. } = step
                && service_exists(service)
            {
                actions.push(Action::WingetUninstall { id: package.clone(), service: service.clone() });
            }
        }
    }
    if actions.is_empty() {
        return Ok(());
    }
    JobFolder::new(id)?.run(actions)
}

/// Clean up / Uninstall all: task and protected folder of each of these programs, with one prompt for all.
pub fn remove_all(ids: &[String]) -> Result<(), String> {
    let mut actions = vec![];
    for id in ids {
        actions.push(Action::RemoveTask { name: task_name(id) });
        actions.push(Action::RemoveDir { dir: protected_dir(id) });
    }
    if actions.is_empty() {
        return Ok(());
    }
    JobFolder::new("toolbox")?.run(actions)
}

/// A temporary folder for the job file, the downloads and the helper's answer; removed when done.
struct JobFolder {
    dir: PathBuf,
}

impl Drop for JobFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl JobFolder {
    fn new(id: &str) -> Result<JobFolder, String> {
        let dir = std::env::temp_dir().join(format!("WindowsToolbox-setup-{id}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(JobFolder { dir })
    }

    fn run(&self, actions: Vec<Action>) -> Result<(), String> {
        let file = self.dir.join("job.json");
        std::fs::write(&file, serde_json::to_vec(&Job { actions }).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        elevate(&file)?;
        let outcomes: Vec<Outcome> = std::fs::read(result_file(&file)).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or_else(|| crate::t!("kit.setup.no_answer"))?;
        match outcomes.into_iter().find(|o| !o.ok) {
            Some(failed) => Err(failed.message),
            None => Ok(()),
        }
    }
}

fn result_file(job: &Path) -> PathBuf {
    job.with_file_name("result.json")
}

/// Starts this exe again with admin rights and waits until it has done the job. Windows asks first.
fn elevate(job: &Path) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError};
    use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let (verb, file) = (wide("runas"), wide(&exe.to_string_lossy()));
    let params = wide(&format!("{HELPER} {}", crate::process::quote(&job.to_string_lossy())));
    let dir = wide(&exe.parent().unwrap_or(Path::new(".")).to_string_lossy());
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.lpDirectory = dir.as_ptr();
    info.nShow = 0;
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            return Err(if GetLastError() == ERROR_CANCELLED { crate::t!("kit.setup.cancelled") } else { io::Error::last_os_error().to_string() });
        }
        if !info.hProcess.is_null() {
            WaitForSingleObject(info.hProcess, INFINITE);
            CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

/// The SID of the user this process runs as ("S-1-5-21-…").
fn user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut buffer = vec![0u8; 256];
        let mut len = 0;
        let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr() as *mut _, buffer.len() as u32, &mut len) != 0;
        CloseHandle(token);
        if !ok {
            return None;
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return None;
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text as *mut _);
        Some(sid)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------- the helper with admin rights ----------

/// `app::init` calls this when the exe was started with `--kit-setup <job>`: does the job, writes the answer next to
/// it and ends the process.
pub(crate) fn helper(job: &Path) -> ! {
    let outcomes = match std::fs::read(job).ok().and_then(|b| serde_json::from_slice::<Job>(&b).ok()) {
        Some(job) => perform(&job),
        None => vec![Outcome { ok: false, message: "the setup job could not be read".into() }],
    };
    let ok = outcomes.iter().all(|o| o.ok);
    let _ = std::fs::write(result_file(job), serde_json::to_vec(&outcomes).unwrap_or_default());
    std::process::exit(if ok { 0 } else { 1 });
}

/// Does the actions in order; stops at the first that fails.
fn perform(job: &Job) -> Vec<Outcome> {
    let mut outcomes = vec![];
    for action in &job.actions {
        let result = allowed(action).and_then(|()| act(action));
        let failed = result.is_err();
        outcomes.push(match result {
            Ok(()) => Outcome { ok: true, message: String::new() },
            Err(message) => Outcome { ok: false, message },
        });
        if failed {
            break;
        }
    }
    outcomes
}

/// The helper writes only below the protected folder and registers only our tasks – whatever the job file says.
fn allowed(action: &Action) -> Result<(), String> {
    let root = protected_root();
    let inside = |p: &Path| {
        let clean = !p.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir));
        let below = p.strip_prefix(&root).is_ok_and(|rest| rest.components().next().is_some());
        if clean && below { Ok(()) } else { Err(format!("not inside {}: {}", root.display(), p.display())) }
    };
    let task = |name: &str| {
        let rest = name.strip_prefix("\\WindowsToolbox\\").unwrap_or_default();
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) { Ok(()) } else { Err(format!("not one of our tasks: {name}")) }
    };
    let package = |id: &str| {
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)) { Ok(()) } else { Err(format!("not a package id: {id}")) }
    };
    match action {
        Action::WingetInstall { id, .. } | Action::WingetUninstall { id, .. } => package(id),
        Action::Unpack { into, .. } => inside(into),
        Action::Copy { to, .. } => inside(to),
        Action::Task { name, run, .. } => task(name).and_then(|()| inside(run)),
        Action::RemoveTask { name } => task(name),
        Action::RemoveDir { dir } => inside(dir),
    }
}

fn act(action: &Action) -> Result<(), String> {
    match action {
        Action::WingetInstall { id, service } => {
            winget(&["install", "--id", id, "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"])?;
            if service_exists(service) { Ok(()) } else { Err(crate::t!("kit.setup.winget_failed", name = id)) }
        }
        Action::WingetUninstall { id, service } => {
            winget(&["uninstall", "--id", id, "--exact", "--silent", "--accept-source-agreements", "--disable-interactivity"])?;
            if service_exists(service) { Err(crate::t!("kit.setup.winget_failed", name = id)) } else { Ok(()) }
        }
        Action::Unpack { zip: file, sha256, into } => {
            let data = std::fs::read(file).map_err(|e| e.to_string())?;
            // Checked again on exactly the bytes that are unpacked – the file lay in a folder the user can write to.
            if !hex(&Sha256::digest(&data)).eq_ignore_ascii_case(sha256) {
                return Err(crate::t!("kit.setup.checksum", name = file.display()));
            }
            let files = zip::read(&data, DOWNLOAD_LIMIT)?;
            stop_under(into);
            if into.exists() {
                std::fs::remove_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
            }
            for (name, content) in files {
                let relative = Path::new(&name);
                if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
                    return Err(format!("a file name in the zip points outside: {name}"));
                }
                let target = into.join(relative);
                if let Some(dir) = target.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&target, content).map_err(|e| format!("{}: {e}", target.display()))?;
            }
            std::fs::write(into.join(STAMP), sha256.to_lowercase()).map_err(|e| e.to_string())
        }
        Action::Copy { from, to } => {
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                stop_under(dir);
            }
            std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("{}: {e}", to.display()))
        }
        Action::Task { name, run, user, description } => {
            let xml = task_xml(run, user, description);
            let file = std::env::temp_dir().join(format!("WindowsToolbox-task-{}.xml", std::process::id()));
            // schtasks wants the XML as UTF-16 with a byte order mark.
            let mut bytes = vec![0xFF, 0xFE];
            bytes.extend(xml.encode_utf16().flat_map(|u| u.to_le_bytes()));
            std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
            let out = hidden("schtasks.exe").args(["/create", "/tn", name, "/xml"]).arg(&file).arg("/f").output();
            let _ = std::fs::remove_file(&file);
            let out = out.map_err(|e| e.to_string())?;
            if out.status.success() { Ok(()) } else { Err(format!("schtasks: {}", String::from_utf8_lossy(&out.stderr).trim())) }
        }
        Action::RemoveTask { name } => {
            let _ = hidden("schtasks.exe").args(["/end", "/tn", name]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            let out = hidden("schtasks.exe").args(["/delete", "/tn", name, "/f"]).stdout(Stdio::null()).output().map_err(|e| e.to_string())?;
            // Not there is as good as removed.
            if out.status.success() || task_command(name).is_none() { Ok(()) } else { Err(format!("schtasks: {}", String::from_utf8_lossy(&out.stderr).trim())) }
        }
        Action::RemoveDir { dir } => {
            stop_under(dir);
            if dir.exists() {
                std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            // The common folder goes with the last program in it.
            let root = protected_root();
            if std::fs::read_dir(&root).is_ok_and(|mut d| d.next().is_none()) {
                let _ = std::fs::remove_dir(&root);
            }
            Ok(())
        }
    }
}

fn winget(args: &[&str]) -> Result<(), String> {
    // winget's own result codes say more than we could; whether it worked is checked by the service afterwards.
    hidden("winget.exe").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|_| ()).map_err(|e| crate::t!("kit.setup.no_winget", why = e))
}

/// A task without a trigger: it runs only when the program starts it, as the user, with their admin rights, for as
/// long as it likes, also on battery, never twice at a time.
fn task_xml(run: &Path, user: &str, description: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{description}</Description>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
    </Exec>
  </Actions>
</Task>
"#,
        description = escape(description),
        user = escape(user),
        command = escape(&run.to_string_lossy()),
    )
}

/// Ends every process whose exe lies in `dir` – a running bridge locks its files.
fn stop_under(dir: &Path) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::Threading::*;
    let prefix = dir.to_string_lossy().to_lowercase();
    let prefix = format!("{}\\", prefix.trim_end_matches('\\'));
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, entry.th32ProcessID);
            if !h.is_null() {
                let mut buffer = [0u16; 1024];
                let mut len = buffer.len() as u32;
                if QueryFullProcessImageNameW(h, 0, buffer.as_mut_ptr(), &mut len) != 0 {
                    let path = String::from_utf16_lossy(&buffer[..len as usize]).to_lowercase();
                    if path.starts_with(&prefix) {
                        TerminateProcess(h, 1);
                        WaitForSingleObject(h, 3000);
                    }
                }
                CloseHandle(h);
            }
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests that move the protected folder must not run beside those that read where it is.
    static ROOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kit-setup-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The helper refuses everything outside the protected folder and our tasks – whatever the job file says.
    #[test]
    fn the_helper_stays_in_its_place() {
        let _lock = ROOT.lock().unwrap_or_else(|e| e.into_inner());
        let root = protected_root();
        let ok = |a: Action| allowed(&a).is_ok();
        assert!(ok(Action::RemoveDir { dir: root.join("hardware_monitor") }));
        assert!(!ok(Action::RemoveDir { dir: root.clone() }), "not the common folder itself");
        assert!(!ok(Action::RemoveDir { dir: root.join("hardware_monitor").join("..").join("..") }));
        assert!(!ok(Action::RemoveDir { dir: PathBuf::from("C:\\Windows") }));
        assert!(!ok(Action::Copy { from: "a".into(), to: PathBuf::from("C:\\Windows\\System32\\x.exe") }));
        assert!(ok(Action::Task { name: "\\WindowsToolbox\\hardware_monitor".into(), run: root.join("hardware_monitor").join("LhmBridge.exe"), user: String::new(), description: String::new() }));
        assert!(!ok(Action::Task { name: "\\Microsoft\\Windows\\x".into(), run: root.join("a").join("b.exe"), user: String::new(), description: String::new() }));
        assert!(!ok(Action::Task { name: "\\WindowsToolbox\\x".into(), run: PathBuf::from("C:\\Users\\me\\evil.exe"), user: String::new(), description: String::new() }));
        assert!(!ok(Action::RemoveTask { name: "\\WindowsToolbox\\..\\x".into() }));
        assert!(!ok(Action::WingetInstall { id: "a b & calc".into(), service: String::new() }));
        assert!(ok(Action::WingetInstall { id: "namazso.PawnIO".into(), service: "PawnIO".into() }));
    }

    /// Unpack, copy and remove as the helper does them – in a folder of the test's own instead of Program Files.
    #[test]
    fn unpack_copy_and_remove() {
        let _lock = ROOT.lock().unwrap_or_else(|e| e.into_inner());
        let base = temp("do");
        let root = base.join("WindowsToolbox");
        unsafe { std::env::set_var(ROOT_VARIABLE, &root) };
        let id = "kit_setup_test";
        let zip_bytes = zip::write(&[("lib.dll".into(), b"library".to_vec()), ("sub/x.txt".into(), b"x".to_vec())]);
        let sha = hex(&Sha256::digest(&zip_bytes));
        let zip_file = base.join("1.zip");
        std::fs::write(&zip_file, &zip_bytes).unwrap();
        let source = base.join("Bridge.exe");
        std::fs::write(&source, b"bridge").unwrap();
        let into = place(id, "lhm");
        let job = Job {
            actions: vec![
                Action::Unpack { zip: zip_file.clone(), sha256: sha.clone(), into: into.clone() },
                Action::Copy { from: source.clone(), to: protected_dir(id).join("Bridge.exe") },
            ],
        };
        assert!(perform(&job).iter().all(|o| o.ok), "{:?}", perform(&job));
        assert_eq!(std::fs::read(into.join("sub").join("x.txt")).unwrap(), b"x");
        let step = Step::Download { name: "LHM".into(), version: "1".into(), url: String::new(), sha256: sha.to_uppercase(), into: "lhm".into() };
        assert_eq!(done(id, &step), Done::Yes);
        let other = Step::Download { name: "LHM".into(), version: "2".into(), url: String::new(), sha256: "00".into(), into: "lhm".into() };
        assert_eq!(done(id, &other), Done::Outdated);

        // A changed download is refused before anything is unpacked.
        std::fs::write(&zip_file, b"something else").unwrap();
        let bad = perform(&Job { actions: vec![Action::Unpack { zip: zip_file, sha256: sha, into: into.clone() }] });
        assert!(!bad[0].ok && into.join("lib.dll").exists(), "{bad:?}");

        // Removing the last program takes the common folder along.
        assert!(perform(&Job { actions: vec![Action::RemoveDir { dir: protected_dir(id) }] })[0].ok);
        assert!(!root.exists());
        unsafe { std::env::remove_var(ROOT_VARIABLE) };
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_task_file() {
        let xml = task_xml(Path::new("C:\\Program Files\\WindowsToolbox\\a & b\\x.exe"), "S-1-5-21-1", "Hardware <Monitor>");
        assert!(xml.contains("<Command>C:\\Program Files\\WindowsToolbox\\a &amp; b\\x.exe</Command>"));
        assert!(xml.contains("<Description>Hardware &lt;Monitor&gt;</Description>"));
        assert!(!xml.contains("<Triggers>"), "no trigger: it runs only when started");
        assert_eq!(unescape(between(&xml, "<Command>", "</Command>").unwrap()), "C:\\Program Files\\WindowsToolbox\\a & b\\x.exe");
    }

    #[test]
    fn the_user_and_a_service() {
        assert!(user_sid().is_some_and(|s| s.starts_with("S-1-5-")));
        assert!(service_exists("EventLog"), "every Windows has it");
        assert!(!service_exists("NoSuchServiceAtAll123"));
    }
}
