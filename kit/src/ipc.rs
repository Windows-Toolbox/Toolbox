//! Named pipes, one JSON object per line in each direction (DESIGN.md 4.3).
//!
//! Handles are opened with FILE_FLAG_OVERLAPPED: on a synchronous handle Windows serialises all I/O, so a reader
//! thread waiting in ReadFile would block every WriteFile from another thread. Each operation gets its own event,
//! and the calling thread waits for it – simple blocking code, but reading and writing can overlap.

use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::*;
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken};

/// The agent's id: its pipe is `windows-toolbox.toolbox`.
pub const TOOLBOX: &str = "toolbox";

/// Tests run next to the programs that are really in use. The internal environment variable `TOOLBOX_SANDBOX`
/// gives every name Windows knows (pipes, jobs) a suffix, so the two never meet. Empty outside of tests.
pub fn sandbox() -> &'static str {
    static SUFFIX: OnceLock<String> = OnceLock::new();
    SUFFIX.get_or_init(|| match std::env::var("TOOLBOX_SANDBOX") {
        Ok(s) if !s.is_empty() => format!(".{s}"),
        _ => String::new(),
    })
}

pub fn pipe_name(id: &str) -> String {
    format!(r"\\.\pipe\windows-toolbox.{id}{}", sandbox())
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Event(HANDLE);
impl Event {
    fn new() -> io::Result<Self> {
        let h = unsafe { CreateEventW(null(), 1, 0, null()) };
        if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Event(h)) }
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Runs one overlapped operation to its end. `op` starts it and returns the BOOL of the API call.
fn overlapped(h: HANDLE, op: impl FnOnce(*mut OVERLAPPED) -> windows_sys::core::BOOL) -> io::Result<u32> {
    let ev = Event::new()?;
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = ev.0;
    if op(&mut ov) == 0 {
        let err = unsafe { GetLastError() };
        if err != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(err as i32));
        }
    }
    let mut n = 0u32;
    if unsafe { GetOverlappedResult(h, &ov, &mut n, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n)
}

/// One end of a pipe connection. Shared through `Arc`; `send` may be called from any thread while another
/// thread is reading.
pub struct Conn {
    h: HANDLE,
    write: Mutex<()>,
    read_buf: Mutex<Vec<u8>>,
}
unsafe impl Send for Conn {}
unsafe impl Sync for Conn {}

impl Conn {
    fn new(h: HANDLE) -> Arc<Conn> {
        Arc::new(Conn { h, write: Mutex::new(()), read_buf: Mutex::new(Vec::new()) })
    }

    pub fn send_line(&self, line: &str) -> io::Result<()> {
        let mut data = line.as_bytes().to_vec();
        data.push(b'\n');
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut off = 0;
        while off < data.len() {
            let chunk = &data[off..];
            let n = overlapped(self.h, |ov| unsafe { WriteFile(self.h, chunk.as_ptr(), chunk.len() as u32, null_mut(), ov) })?;
            off += n as usize;
        }
        Ok(())
    }

    pub fn send<T: serde::Serialize>(&self, msg: &T) -> io::Result<()> {
        self.send_line(&serde_json::to_string(msg).map_err(io::Error::other)?)
    }

    /// Next line, or `None` when the other side closed the pipe (or `close` was called).
    pub fn read_line(&self) -> Option<String> {
        let mut buf = self.read_buf.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                return Some(String::from_utf8_lossy(&line[..line.len() - 1]).trim_end_matches('\r').to_string());
            }
            let mut chunk = [0u8; 16 * 1024];
            let r = overlapped(self.h, |ov| unsafe { ReadFile(self.h, chunk.as_mut_ptr(), chunk.len() as u32, null_mut(), ov) });
            match r {
                Ok(0) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n as usize]),
                Err(e) if e.raw_os_error() == Some(ERROR_MORE_DATA as i32) => continue,
                Err(_) => return None,
            }
        }
    }

    /// The next message of type `T`; lines that are no valid `T` are skipped. `None` when the pipe closed.
    pub fn read<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        loop {
            if let Ok(msg) = serde_json::from_str(&self.read_line()?) {
                return Some(msg);
            }
        }
    }

    /// Makes a `read_line` waiting in another thread return `None`.
    pub fn close(&self) {
        unsafe { CancelIoEx(self.h, null()) };
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // Only closed, never disconnected: what was written before (a last `bye`) can still be read by the other side.
        unsafe { CloseHandle(self.h) };
    }
}

/// The logged-in user's SID as text (S-1-5-21-…). An elevated process of the same user has the same SID.
pub fn current_user_sid() -> io::Result<String> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut len);
        CloseHandle(token);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: *mut u16 = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
            return Err(io::Error::last_os_error());
        }
        let len = (0..).take_while(|&i| *s.add(i) != 0).count();
        let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
        LocalFree(s as *mut c_void);
        Ok(out)
    }
}

struct SecDesc(PSECURITY_DESCRIPTOR);
impl Drop for SecDesc {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

/// Only the logged-in user may open the pipe ("P" = no inherited entries, so nobody else is let in).
fn user_only_descriptor() -> io::Result<SecDesc> {
    let sddl = wide(&format!("D:P(A;;GA;;;{})", current_user_sid()?));
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(SecDesc(sd))
}

fn create_instance(name: &[u16], first: bool, sd: &SecDesc) -> io::Result<HANDLE> {
    let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: 0 };
    let mut open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &sa,
        )
    };
    if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(h) }
}

/// Holds the pipe name. Binding fails with `AlreadyExists` if another process holds it – the single-instance
/// lock (4.3).
pub struct Server {
    name: Vec<u16>,
    sd: SecDesc,
    /// The instance that waits for the next client. There is always one, so the name never disappears while the
    /// server lives – a client finds either a free instance or "busy", never "not there".
    next: Mutex<HANDLE>,
}
unsafe impl Send for Server {}
unsafe impl Sync for Server {}

impl Server {
    pub fn bind(id: &str) -> io::Result<Server> {
        let name = wide(&pipe_name(id));
        let sd = user_only_descriptor()?;
        match create_instance(&name, true, &sd) {
            Ok(h) => Ok(Server { name, sd, next: Mutex::new(h) }),
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) || e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                Err(io::Error::new(io::ErrorKind::AlreadyExists, e))
            }
            Err(e) => Err(e),
        }
    }

    /// Waits for the next client.
    pub fn accept(&self) -> io::Result<Arc<Conn>> {
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let h = *next;
        let r = overlapped(h, |ov| unsafe { ConnectNamedPipe(h, ov) });
        match r {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) => {}
            Err(e) => return Err(e),
        }
        // The new waiting instance exists before the connected one is handed out.
        *next = create_instance(&self.name, false, &self.sd)?;
        Ok(Conn::new(h))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        unsafe { CloseHandle(*self.next.lock().unwrap_or_else(|e| e.into_inner())) };
    }
}

/// Opens a connection to the program `id`. `NotFound` if it is not running.
pub fn connect(id: &str) -> io::Result<Arc<Conn>> {
    let name = wide(&pipe_name(id));
    for _ in 0..50 {
        let h = unsafe { CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, null_mut()) };
        if h != INVALID_HANDLE_VALUE {
            return Ok(Conn::new(h));
        }
        let err = unsafe { GetLastError() };
        if err != ERROR_PIPE_BUSY {
            return Err(io::Error::from_raw_os_error(err as i32));
        }
        unsafe { WaitNamedPipeW(name.as_ptr(), 100) };
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "pipe busy"))
}

/// Like `connect`, but waits up to `timeout` for the pipe to appear – for a program that was just started.
pub fn connect_wait(id: &str, timeout: std::time::Duration) -> io::Result<Arc<Conn>> {
    let t = std::time::Instant::now();
    loop {
        match connect(id) {
            Ok(c) => return Ok(c),
            Err(e) if t.elapsed() >= timeout => return Err(e),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

/// Does the pipe exist (= is the program's process there)? Without connecting, so the program sees no client.
pub fn exists(id: &str) -> bool {
    let name = wide(&pipe_name(id));
    // WaitNamedPipe with a timeout of 1 ms returns at once: true if an instance is free, ERROR_SEM_TIMEOUT if all
    // are busy (still exists), ERROR_FILE_NOT_FOUND if there is none.
    let ok = unsafe { WaitNamedPipeW(name.as_ptr(), 1) };
    ok != 0 || unsafe { GetLastError() } != ERROR_FILE_NOT_FOUND
}

/// The Python programs' way of talking, until the last one is ported (DESIGN.md 11, M1): a TCP port on
/// 127.0.0.1 named by `"port"` in their manifest, one command per connection, one JSON line as the answer.
pub mod legacy {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::time::Duration;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    /// Is somebody listening on the port? Asked by trying to take the port ourselves, which fails at once if it is
    /// taken – connecting to a closed port instead would wait for its timeout.
    pub fn listening(port: u16) -> bool {
        TcpListener::bind(addr(port)).is_err()
    }

    /// Sends one command (`status`, `quit`, `reload`, `show <id>` …). `None` if nobody answered; the Toolbox's own
    /// port answers `ok`, which comes back as the string `"ok"`.
    pub fn command(port: u16, command: &str, timeout: Duration) -> Option<serde_json::Value> {
        let mut conn = TcpStream::connect_timeout(&addr(port), timeout).ok()?;
        conn.set_read_timeout(Some(timeout)).ok()?;
        conn.write_all(format!("{command}\n").as_bytes()).ok()?;
        let mut line = String::new();
        BufReader::new(conn).read_line(&mut line).ok()?;
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        Some(serde_json::from_str(line).unwrap_or_else(|_| serde_json::Value::String(line.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_both_ways_and_single_instance() {
        let id = format!("kit-test-{}", std::process::id());
        assert!(!exists(&id));
        let server = Server::bind(&id).unwrap();
        assert!(exists(&id));
        assert_eq!(Server::bind(&id).err().map(|e| e.kind()), Some(io::ErrorKind::AlreadyExists));

        let t = std::thread::spawn(move || {
            let c = server.accept().unwrap();
            let line = c.read_line().unwrap();
            c.send_line(&format!("got {line}")).unwrap();
            // A reader waiting for the next line must not block this write.
            let c2 = c.clone();
            let reader = std::thread::spawn(move || c2.read_line());
            std::thread::sleep(std::time::Duration::from_millis(50));
            c.send_line("second").unwrap();
            reader.join().unwrap()
        });
        let c = connect(&id).unwrap();
        c.send_line("häuser ●").unwrap();
        assert_eq!(c.read_line().as_deref(), Some("got häuser ●"));
        assert_eq!(c.read_line().as_deref(), Some("second"));
        drop(c);
        assert_eq!(t.join().unwrap(), None);
        assert!(!exists(&id), "the name is gone with the server");
    }

    #[test]
    fn the_old_port_way() {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(legacy::listening(port));
        let t = std::thread::spawn(move || {
            for answer in ["{\"ok\": true, \"pid\": 42}\n", "ok\n"] {
                let (mut conn, _) = listener.accept().unwrap();
                let mut line = String::new();
                std::io::BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
                conn.write_all(answer.as_bytes()).unwrap();
            }
        });
        let timeout = std::time::Duration::from_millis(500);
        assert_eq!(legacy::command(port, "status", timeout).unwrap()["pid"], 42);
        assert_eq!(legacy::command(port, "show x", timeout).unwrap(), "ok");
        t.join().unwrap();
        assert!(!legacy::listening(port));
        assert!(legacy::command(port, "status", std::time::Duration::from_millis(100)).is_none());
    }
}
