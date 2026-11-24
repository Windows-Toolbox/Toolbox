//! What the Toolbox leaves in Windows, and how it takes it out again (DESIGN.md 8, 9): its own "Start with Windows",
//! the Start menu shortcut, and Gear → Clean up / Uninstall all. Everything it writes there carries the name
//! `WindowsToolbox`, so it can be found again without a list of its own.

use std::path::{Path, PathBuf};

use kit::{autostart, ipc};
use serde::{Deserialize, Serialize};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoUninitialize, IPersistFile, STGM_READ};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink};
use windows::core::{HSTRING, Interface, PWSTR};

use crate::library;

/// The application id every process of the Toolbox carries (`kit::app::init`); the shortcut carries it too, so
/// Windows takes the shortcut and the running Toolbox for one program (taskbar, pinning).
const AUMID: &str = "WindowsToolbox.toolbox";

/// Where the Start menu shortcut lies. In the sandbox a folder of its own – tests never touch the real Start menu.
pub fn shortcut_path() -> PathBuf {
    let dir = if ipc::sandbox().is_empty() {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(r"Microsoft\Windows\Start Menu\Programs")
    } else {
        kit::app::data_dir().join(format!("start-menu{}", ipc::sandbox()))
    };
    dir.join("Windows Toolbox.lnk")
}

/// COM wants a thread that has said which kind it is; a short thread of its own is the simplest such thread.
fn with_com<R: Send + 'static>(f: impl FnOnce() -> windows::core::Result<R> + Send + 'static) -> Result<R, String> {
    std::thread::spawn(move || {
        let ready = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let result = f().map_err(|e| e.message());
        if ready {
            unsafe { CoUninitialize() };
        }
        result
    })
    .join()
    .unwrap_or_else(|_| Err("COM".into()))
}

/// The exe a shortcut starts – `None` if there is no shortcut (or it cannot be read).
pub fn shortcut_target(lnk: &Path) -> Option<PathBuf> {
    if !lnk.is_file() {
        return None;
    }
    let lnk = HSTRING::from(lnk.as_os_str());
    with_com(move || unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.cast::<IPersistFile>()?.Load(&lnk, STGM_READ)?;
        let mut path = [0u16; 1024];
        link.GetPath(&mut path, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
        let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        Ok(PathBuf::from(String::from_utf16_lossy(&path[..len])))
    })
    .ok()
    .filter(|p| !p.as_os_str().is_empty())
}

/// Writes the shortcut: it starts `exe` in its folder, with its icon and the Toolbox's application id.
pub fn write_shortcut(lnk: &Path, exe: &Path) -> Result<(), String> {
    if let Some(dir) = lnk.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let (lnk, target) = (HSTRING::from(lnk.as_os_str()), HSTRING::from(exe.as_os_str()));
    let folder = HSTRING::from(exe.parent().unwrap_or(exe).as_os_str());
    with_com(move || unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&target)?;
        link.SetWorkingDirectory(&folder)?;
        link.SetIconLocation(&target, 0)?;
        let store: IPropertyStore = link.cast()?;
        // The shell wants the id as a plain string (VT_LPWSTR). Its memory comes from COM, because dropping the
        // value hands it back to COM.
        let id: Vec<u16> = AUMID.encode_utf16().chain([0]).collect();
        let memory = CoTaskMemAlloc(id.len() * 2) as *mut u16;
        if memory.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(id.as_ptr(), memory, id.len());
        let mut value = PROPVARIANT::default();
        (*value.Anonymous.Anonymous).vt = VT_LPWSTR;
        (*value.Anonymous.Anonymous).Anonymous.pwszVal = PWSTR(memory);
        store.SetValue(&PKEY_AppUserModel_ID, &value)?;
        store.Commit()?;
        link.cast::<IPersistFile>()?.Save(&lnk, true)?;
        Ok(())
    })
}

/// A target that is no longer there: its program was deleted or moved.
fn gone(target: Option<&Path>) -> bool {
    target.is_none_or(|t| !t.exists())
}

/// Something the Toolbox or one of its programs put into Windows.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trace {
    /// A "Start with Windows" entry: the program's id and the exe it starts.
    Autostart { id: String, target: Option<PathBuf> },
    /// The Start menu shortcut and the exe it starts.
    Shortcut { path: PathBuf, target: Option<PathBuf> },
    /// What a program set up with admin rights (kit::setup, DESIGN.md 9): its task `\WindowsToolbox\<id>` and
    /// its folder in `C:\Program Files\WindowsToolbox\`.
    Setup { id: String },
}

impl Trace {
    pub fn target(&self) -> Option<&Path> {
        match self {
            Trace::Autostart { target, .. } | Trace::Shortcut { target, .. } => target.as_deref(),
            Trace::Setup { .. } => None,
        }
    }
}

/// What there is. `everything`: all of it (Uninstall all); otherwise only what belongs to no program any more – its
/// exe is gone, or, for a setup, its id is not on the list (`listed`) (Clean up). A moved program's entry is not
/// among them: the agent points it at the new place first.
pub fn traces(everything: bool, listed: &[String]) -> Vec<Trace> {
    let mut found = vec![];
    for id in autostart::all() {
        let target = autostart::read(&id).target;
        if everything || gone(target.as_deref()) {
            found.push(Trace::Autostart { id, target });
        }
    }
    let path = shortcut_path();
    if path.is_file() {
        let target = shortcut_target(&path);
        if everything || gone(target.as_deref()) {
            found.push(Trace::Shortcut { path, target });
        }
    }
    for id in kit::setup::installed() {
        if everything || !listed.contains(&id) {
            found.push(Trace::Setup { id });
        }
    }
    found
}

/// Takes traces out of Windows – all setups together, so Windows asks for admin rights only once. The reasons of
/// what failed.
pub fn remove_all(traces: &[Trace]) -> (usize, Vec<String>) {
    let (mut removed, mut failed) = (0, vec![]);
    let setups: Vec<String> = traces.iter().filter_map(|t| if let Trace::Setup { id } = t { Some(id.clone()) } else { None }).collect();
    for trace in traces.iter().filter(|t| !matches!(t, Trace::Setup { .. })) {
        match remove(trace) {
            Ok(()) => removed += 1,
            Err(why) => failed.push(why),
        }
    }
    if !setups.is_empty() {
        match kit::setup::remove_all(&setups) {
            Ok(()) => removed += setups.len(),
            Err(why) => failed.push(why),
        }
    }
    (removed, failed)
}

/// Takes one trace out of Windows.
pub fn remove(trace: &Trace) -> Result<(), String> {
    match trace {
        Trace::Autostart { id, .. } => autostart::set(id, None).map_err(|e| e.to_string()),
        Trace::Shortcut { path, .. } => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
        Trace::Setup { id } => kit::setup::remove_all(std::slice::from_ref(id)),
    }
}

/// What the Toolbox itself has in Windows – shown in the gear.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Own {
    /// The Toolbox's exe (by its name – `build.ps1` may have put the running one aside).
    pub exe: PathBuf,
    pub autostart: autostart::Entry,
    /// The exe the Start menu shortcut starts; `None`: there is no shortcut.
    pub shortcut: Option<PathBuf>,
}

impl Own {
    /// `target` starts this copy – or an exe that is gone (a moved folder; repaired at the agent's start).
    fn mine(&self, target: Option<&Path>) -> bool {
        target.is_some_and(|t| !t.exists() || library::same_file(t, &self.exe))
    }

    pub fn autostart_is_mine(&self) -> bool {
        self.mine(self.autostart.target.as_deref())
    }

    pub fn autostart_on(&self) -> bool {
        self.autostart.state == autostart::State::On && self.autostart_is_mine()
    }

    pub fn shortcut_on(&self) -> bool {
        self.shortcut.is_some() && self.shortcut_is_mine()
    }

    pub fn shortcut_is_mine(&self) -> bool {
        self.mine(self.shortcut.as_deref())
    }
}

/// Reads what Windows has noted for the Toolbox (the shortcut through COM – not once a second).
pub fn own(exe: &Path) -> Own {
    Own { exe: exe.to_path_buf(), autostart: autostart::read(library::OWN_ID), shortcut: shortcut_target(&shortcut_path()) }
}

/// After the Toolbox's folder was moved: what still names an exe that is gone is pointed at `exe`. What names an
/// exe that exists belongs to another copy and is left alone.
pub fn repair_own(exe: &Path) {
    autostart::repair(library::OWN_ID, exe);
    let path = shortcut_path();
    if path.is_file() && gone(shortcut_target(&path).as_deref()) {
        if let Err(e) = write_shortcut(&path, exe) {
            kit::app::log(&format!("the Start menu shortcut could not be repaired: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real shortcut, written and read back – into a folder of the test, never the Start menu.
    #[test]
    fn a_shortcut_both_ways() {
        let dir = std::env::temp_dir().join(format!("toolbox-shortcut-{}", std::process::id()));
        let lnk = dir.join("Test.lnk");
        assert_eq!(shortcut_target(&lnk), None, "no shortcut");
        let exe = std::env::current_exe().unwrap();
        write_shortcut(&lnk, &exe).unwrap();
        assert!(library::same_file(&shortcut_target(&lnk).unwrap(), &exe));
        // Pointed somewhere else and back, like a repair after a move.
        let gone = PathBuf::from(r"C:\No Such Folder\Toolbox.exe");
        write_shortcut(&lnk, &gone).unwrap();
        assert_eq!(shortcut_target(&lnk).as_deref(), Some(gone.as_path()));
        let own = Own { exe: exe.clone(), autostart: Default::default(), shortcut: shortcut_target(&lnk) };
        assert!(own.shortcut_on() && own.shortcut_is_mine(), "an exe that is gone counts as this copy's");
        remove(&Trace::Shortcut { path: lnk.clone(), target: None }).unwrap();
        remove(&Trace::Shortcut { path: lnk.clone(), target: None }).unwrap();
        assert!(!lnk.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn whose_entry_it_is() {
        let exe = std::env::current_exe().unwrap();
        let entry = |target: Option<PathBuf>| Own { exe: exe.clone(), autostart: autostart::Entry { state: autostart::State::On, target }, shortcut: None };
        assert!(entry(Some(exe.clone())).autostart_on());
        assert!(!entry(None).autostart_on());
        let other = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("ping.exe");
        assert!(!entry(Some(other)).autostart_on(), "another copy's entry");
        assert!(gone(None) && gone(Some(Path::new(r"C:\No Such Folder\x.exe"))) && !gone(Some(&exe)));
    }
}
