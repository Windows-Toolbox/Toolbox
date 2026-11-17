//! "Start with Windows" (DESIGN.md 5, 8): the Run key of the logged-in user, through direct registry calls – no
//! admin rights, no hidden PowerShell. The entry's name derives from the program's `id`, so nothing has to match
//! anywhere else.
//!
//! Windows can switch an entry off without removing it (Task Manager → Startup, Settings → Apps → Startup). It
//! notes that in a second key; `state` reads both, so a switch never claims "on" for an entry that will not start.

use std::io;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use serde::{Deserialize, Serialize};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::*;

use crate::ipc::{sandbox, wide};

/// Internal argument (never typed by a user): the program was started by Windows at logon, not by a double-click –
/// it starts its work but shows no window.
pub const ARGUMENT: &str = "--autostart";

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Off,
    On,
    /// The entry is there, but Windows will not start it: it was switched off in Task Manager or in Settings.
    DisabledByWindows,
}

/// What Windows has noted for a program.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Entry {
    pub state: State,
    /// The exe the entry starts – `None` without an entry.
    pub target: Option<PathBuf>,
}

/// Tests must never write a real autostart entry: in the sandbox both keys lie somewhere Windows does not look.
fn key_path(real: &str) -> String {
    if sandbox().is_empty() { real.to_string() } else { format!(r"Software\WindowsToolbox\Sandbox{}\{}", sandbox(), real.rsplit('\\').take(2).collect::<Vec<_>>().join("-")) }
}

fn value_name(id: &str) -> String {
    format!("WindowsToolbox.{id}")
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) }
}

/// Opens a key below HKEY_CURRENT_USER; `write` also creates it.
fn open(path: &str, write: bool) -> io::Result<Key> {
    let mut key: HKEY = null_mut();
    let path = wide(path);
    let code = unsafe {
        if write {
            RegCreateKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE, null(), &mut key, null_mut())
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key)
        }
    };
    check(code).map(|_| Key(key))
}

/// A value's type and bytes – `None` if the key or the value is not there.
fn query(path: &str, name: &str) -> Option<(u32, Vec<u8>)> {
    let key = open(path, false).ok()?;
    let name = wide(name);
    let (mut kind, mut size) = (0u32, 0u32);
    if unsafe { RegQueryValueExW(key.0, name.as_ptr(), null(), &mut kind, null_mut(), &mut size) } != ERROR_SUCCESS {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { RegQueryValueExW(key.0, name.as_ptr(), null(), &mut kind, data.as_mut_ptr(), &mut size) } != ERROR_SUCCESS {
        return None;
    }
    data.truncate(size as usize);
    Some((kind, data))
}

fn delete(path: &str, name: &str) -> io::Result<()> {
    let Ok(key) = open(path, true) else { return Ok(()) };
    match unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        code => check(code),
    }
}

/// The command line of an entry: the exe in quotes, then the internal argument.
fn command(exe: &Path) -> String {
    format!("\"{}\" {ARGUMENT}", exe.display())
}

/// The exe a command line starts: what stands between the first pair of quotes, or up to the first space.
fn target_of(command: &str) -> PathBuf {
    let command = command.trim();
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default().into(),
        None => command.split(' ').next().unwrap_or_default().into(),
    }
}

/// What Windows has noted for the program `id`.
pub fn read(id: &str) -> Entry {
    let name = value_name(id);
    let Some((kind, data)) = query(&key_path(RUN), &name).filter(|(kind, _)| *kind == REG_SZ || *kind == REG_EXPAND_SZ) else {
        return Entry { state: State::Off, target: None };
    };
    let _ = kind;
    let text: Vec<u16> = data.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|&c| c != 0).collect();
    let target = target_of(&String::from_utf16_lossy(&text));
    // Windows' note: binary, and an odd first byte means "switched off" (2 and 6 are on, 3 and 7 off). No note: on.
    let disabled = query(&key_path(APPROVED), &name).is_some_and(|(_, note)| note.first().is_some_and(|b| b & 1 == 1));
    Entry { state: if disabled { State::DisabledByWindows } else { State::On }, target: Some(target) }
}

pub fn state(id: &str) -> State {
    read(id).state
}

/// `Some(exe)`: start this exe with Windows – also switches an entry back on that Windows had switched off.
/// `None`: do not start it; the entry and Windows' note are removed.
pub fn set(id: &str, exe: Option<&Path>) -> io::Result<()> {
    let name = value_name(id);
    match exe {
        Some(exe) => {
            let key = open(&key_path(RUN), true)?;
            let data = wide(&command(exe));
            check(unsafe { RegSetValueExW(key.0, wide(&name).as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) })?;
        }
        None => delete(&key_path(RUN), &name)?,
    }
    delete(&key_path(APPROVED), &name)
}

/// After the folder was moved (DESIGN.md 2, "portable stays"): if there is an entry and the exe it names **no longer
/// exists**, it is pointed at `exe`. An entry whose exe exists is left alone – so two copies of one program (your
/// own build and a release) do not take the entry away from each other. Windows' "switched off" is kept.
/// `true` if the entry was changed.
pub fn repair(id: &str, exe: &Path) -> bool {
    let name = value_name(id);
    let entry = read(id);
    let Some(target) = entry.target.filter(|t| !t.exists()) else { return false };
    let _ = target;
    let Ok(key) = open(&key_path(RUN), true) else { return false };
    let data = wide(&command(exe));
    unsafe { RegSetValueExW(key.0, wide(&name).as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) == ERROR_SUCCESS }
}

/// Takes over the entry an older version of a program made under another name (the Python programs: `"autostart"`
/// in their manifest): if it exists, it is removed, and – unless there is an entry of ours already – `exe` starts
/// with Windows instead. `true` if there was such an entry.
pub fn take_over(old_name: &str, id: &str, exe: &Path) -> bool {
    if query(&key_path(RUN), old_name).is_none() {
        return false;
    }
    let was_disabled = query(&key_path(APPROVED), old_name).is_some_and(|(_, note)| note.first().is_some_and(|b| b & 1 == 1));
    if read(id).state == State::Off && !was_disabled {
        let _ = set(id, Some(exe));
    }
    let _ = delete(&key_path(RUN), old_name);
    let _ = delete(&key_path(APPROVED), old_name);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests write to the registry – below a key of their own, never where Windows looks.
    fn sandboxed() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(key_path(RUN).starts_with(r"Software\WindowsToolbox\Sandbox."));
        guard
    }

    fn forget() {
        let path = wide(&format!(r"Software\WindowsToolbox\Sandbox{}", sandbox()));
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
    }

    fn note(id: &str, first_byte: u8) {
        let key = open(&key_path(APPROVED), true).unwrap();
        let data = [first_byte, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        check(unsafe { RegSetValueExW(key.0, wide(&value_name(id)).as_ptr(), 0, REG_BINARY, data.as_ptr(), 12) }).unwrap();
    }

    #[test]
    fn on_off_and_switched_off_by_windows() {
        let _guard = sandboxed();
        forget();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(read("demo"), Entry { state: State::Off, target: None });
        set("demo", Some(&exe)).unwrap();
        assert_eq!(read("demo"), Entry { state: State::On, target: Some(exe.clone()) });
        assert_eq!(state("other"), State::Off, "one program's entry is not another's");

        // What Task Manager writes when the entry is disabled there – and enabled again.
        note("demo", 3);
        assert_eq!(state("demo"), State::DisabledByWindows);
        note("demo", 2);
        assert_eq!(state("demo"), State::On);
        note("demo", 7);
        assert_eq!(state("demo"), State::DisabledByWindows);
        // Switching it on in the program switches it on for Windows too.
        set("demo", Some(&exe)).unwrap();
        assert_eq!(state("demo"), State::On);

        set("demo", None).unwrap();
        assert_eq!(state("demo"), State::Off);
        set("demo", None).unwrap();
        forget();
    }

    #[test]
    fn a_moved_folder_is_repaired_but_another_copy_keeps_its_entry() {
        let _guard = sandboxed();
        forget();
        let here = std::env::current_exe().unwrap();
        let gone = PathBuf::from(r"C:\No Such Folder\Demo.exe");
        assert!(!repair("demo", &here), "no entry, nothing to repair – autostart is never switched on by itself");
        assert_eq!(state("demo"), State::Off);

        set("demo", Some(&gone)).unwrap();
        note("demo", 3);
        assert!(repair("demo", &here));
        assert_eq!(read("demo"), Entry { state: State::DisabledByWindows, target: Some(here.clone()) }, "Windows' choice survives the repair");

        // The entry names an exe that exists: another copy of the program does not take it.
        let other = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("ping.exe");
        assert!(!repair("demo", &other));
        assert_eq!(read("demo").target, Some(here));
        forget();
    }

    #[test]
    fn an_old_entry_under_another_name_is_taken_over() {
        let _guard = sandboxed();
        forget();
        let exe = std::env::current_exe().unwrap();
        assert!(!take_over("DiscordPlus", "discord_plus", &exe));
        assert_eq!(state("discord_plus"), State::Off);

        let key = open(&key_path(RUN), true).unwrap();
        let old = wide("\"C:\\Old\\DiscordPlus.exe\"");
        check(unsafe { RegSetValueExW(key.0, wide("DiscordPlus").as_ptr(), 0, REG_SZ, old.as_ptr() as *const u8, (old.len() * 2) as u32) }).unwrap();
        assert!(take_over("DiscordPlus", "discord_plus", &exe));
        assert_eq!(read("discord_plus"), Entry { state: State::On, target: Some(exe.clone()) });
        assert!(query(&key_path(RUN), "DiscordPlus").is_none(), "the old entry is gone");
        assert!(!take_over("DiscordPlus", "discord_plus", &exe));
        forget();
    }

    #[test]
    fn command_lines() {
        assert_eq!(command(Path::new(r"C:\My Programs\Demo.exe")), "\"C:\\My Programs\\Demo.exe\" --autostart");
        assert_eq!(target_of("\"C:\\My Programs\\Demo.exe\" --autostart"), PathBuf::from(r"C:\My Programs\Demo.exe"));
        assert_eq!(target_of(r"C:\Demo.exe --x"), PathBuf::from(r"C:\Demo.exe"));
    }
}
