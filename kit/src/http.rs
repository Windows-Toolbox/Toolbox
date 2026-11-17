//! HTTPS requests through WinHTTP, the client that is part of Windows: no crate, no certificate store of our own,
//! and the proxy settings of the PC apply. Only what updates (GitHub) and a program's login need: `get` and a
//! form `post`, the whole answer in memory.
//!
//! Only `https://` addresses are taken, and nothing about the user or the PC is sent along: the user agent is the
//! fixed text below.

use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Networking::WinHttp::*;

use crate::ipc::wide;

const USER_AGENT: &str = "windows-toolbox";
/// Resolve, connect, send, receive – each in milliseconds.
const TIMEOUTS: [i32; 4] = [10_000, 10_000, 15_000, 30_000];

pub struct Response {
    /// 200, 404, …
    pub status: u32,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// `limit`: the answer may be at most this many bytes – more is an error, not a full disk or full memory.
pub fn get(url: &str, headers: &[(&str, &str)], limit: usize) -> io::Result<Response> {
    request("GET", url, headers, &[], limit)
}

/// Sends `fields` the way a web form does (`application/x-www-form-urlencoded`).
pub fn post_form(url: &str, fields: &[(&str, &str)], limit: usize) -> io::Result<Response> {
    let body = fields.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&");
    request("POST", url, &[("Content-Type", "application/x-www-form-urlencoded")], body.as_bytes(), limit)
}

/// Percent-encoding for a form field or a part of an address.
pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `https://host[:port]/path?query` → host, port, path with query.
fn split(url: &str) -> io::Result<(String, u16, String)> {
    let bad = || io::Error::new(io::ErrorKind::InvalidInput, format!("not an https address: {url}"));
    let rest = url.strip_prefix("https://").ok_or_else(bad)?;
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().map_err(|_| bad())?),
        None => (authority, 443),
    };
    // No user names in the address, nothing that is not a plain host name.
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(bad());
    }
    Ok((host.to_string(), port, path))
}

struct Handle(*mut c_void);
impl Handle {
    fn new(h: *mut c_void) -> io::Result<Handle> {
        if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Handle(h)) }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn request(verb: &str, url: &str, headers: &[(&str, &str)], body: &[u8], limit: usize) -> io::Result<Response> {
    let (host, port, path) = split(url)?;
    let header_text: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    unsafe {
        let session = Handle::new(WinHttpOpen(wide(USER_AGENT).as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0))?;
        check(WinHttpSetTimeouts(session.0, TIMEOUTS[0], TIMEOUTS[1], TIMEOUTS[2], TIMEOUTS[3]))?;
        let connection = Handle::new(WinHttpConnect(session.0, wide(&host).as_ptr(), port, 0))?;
        // WINHTTP_FLAG_SECURE: TLS, with the certificate checked against Windows' own store. A redirect to a plain
        // http address is refused by WinHTTP's default policy.
        let req = Handle::new(WinHttpOpenRequest(connection.0, wide(verb).as_ptr(), wide(&path).as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE))?;
        let header_wide = wide(&header_text);
        let (header_ptr, header_len) = if header_text.is_empty() { (null(), 0) } else { (header_wide.as_ptr(), (header_wide.len() - 1) as u32) };
        let body_ptr = if body.is_empty() { null() } else { body.as_ptr() as *const c_void };
        check(WinHttpSendRequest(req.0, header_ptr, header_len, body_ptr, body.len() as u32, body.len() as u32, 0))?;
        check(WinHttpReceiveResponse(req.0, null_mut()))?;

        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        check(WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, null(), &mut status as *mut u32 as *mut c_void, &mut size, null_mut()))?;

        let mut out = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            check(WinHttpReadData(req.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read))?;
            if read == 0 {
                break;
            }
            if out.len() + read as usize > limit {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("the answer of {host} is larger than {limit} bytes")));
            }
            out.extend_from_slice(&chunk[..read as usize]);
        }
        Ok(Response { status, body: out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(split("https://api.github.com/repos/a/b?x=1").unwrap(), ("api.github.com".into(), 443, "/repos/a/b?x=1".into()));
        assert_eq!(split("https://example.org").unwrap(), ("example.org".into(), 443, "/".into()));
        assert_eq!(split("https://example.org:8443?x").unwrap(), ("example.org".into(), 8443, "/?x".into()));
        for bad in ["http://example.org/", "https://user@example.org/", "https:///x", "file://c/x", "https://a b/"] {
            assert!(split(bad).is_err(), "{bad}");
        }
        assert_eq!(encode("a b&c=d/ä"), "a%20b%26c%3Dd%2F%C3%A4");
    }
}
