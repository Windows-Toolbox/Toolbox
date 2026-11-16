//! Writes a Windows resource file (`.res`) by hand: the icon, the texts Task Manager and Explorer show (name,
//! description, version) and the application manifest. The linker takes a `.res` as it is, so no resource
//! compiler (`rc.exe`, part of the Windows SDK) has to be installed – Rust's own tools are enough.

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const LANG_EN_US: u16 = 0x0409;

/// What Windows shows about the exe.
pub struct Version<'a> {
    /// "File description" – the name in Task Manager's process list.
    pub description: &'a str,
    pub product: &'a str,
    pub company: &'a str,
    /// The exe's file name.
    pub file_name: &'a str,
    /// `1.2.3` – up to four numbers; anything after a `-` or `+` is ignored.
    pub version: &'a str,
}

/// Declares what every exe of ours wants from Windows: the current look of message boxes and dialogs, sharp
/// drawing on every monitor (per-monitor DPI), no admin rights.
const APP_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </asmv3:windowsSettings>
  </asmv3:application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

/// The whole `.res` file. `icon` is the content of an `.ico` file.
pub fn build(icon: Option<&[u8]>, version: &Version) -> Result<Vec<u8>, String> {
    let mut out = vec![];
    // Every .res file starts with an empty entry.
    resource(&mut out, 0, 0, 0, 0, &[]);
    if let Some(ico) = icon {
        icons(&mut out, ico)?;
    }
    resource(&mut out, RT_VERSION, 1, 0x0030, LANG_EN_US, &version_info(version));
    resource(&mut out, RT_MANIFEST, 1, 0x0030, LANG_EN_US, APP_MANIFEST.as_bytes());
    Ok(out)
}

fn u16le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn pad4(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// One entry: a header (sizes, type and name as numbers, flags, language), then the data, padded to 4 bytes.
fn resource(out: &mut Vec<u8>, kind: u16, name: u16, memory_flags: u16, language: u16, data: &[u8]) {
    u32le(out, data.len() as u32);
    u32le(out, 32); // size of this header
    u16le(out, 0xffff); // "the type is a number"
    u16le(out, kind);
    u16le(out, 0xffff); // "the name is a number"
    u16le(out, name);
    u32le(out, 0); // data version
    u16le(out, memory_flags);
    u16le(out, language);
    u32le(out, 0); // version
    u32le(out, 0); // characteristics
    out.extend_from_slice(data);
    pad4(out);
}

/// An `.ico` file holds several pictures (16, 32, 48 … pixels). Each becomes an entry of its own, and a group
/// entry lists them – Windows picks the fitting size from the group.
fn icons(out: &mut Vec<u8>, ico: &[u8]) -> Result<(), String> {
    let word = |at: usize| ico.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let dword = |at: usize| ico.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if word(0) != Some(0) || word(2) != Some(1) {
        return Err("the icon is no .ico file".into());
    }
    let count = word(4).unwrap_or(0) as usize;
    if count == 0 {
        return Err("the .ico file holds no picture".into());
    }
    let mut group = vec![];
    u16le(&mut group, 0);
    u16le(&mut group, 1);
    u16le(&mut group, count as u16);
    for i in 0..count {
        let entry = 6 + i * 16;
        let (size, offset) = match (dword(entry + 8), dword(entry + 12)) {
            (Some(s), Some(o)) => (s as usize, o as usize),
            _ => return Err("the .ico file is cut off".into()),
        };
        let image = ico.get(offset..offset + size).ok_or("the .ico file is cut off")?;
        let id = (i + 1) as u16;
        resource(out, RT_ICON, id, 0x1010, LANG_EN_US, image);
        // The group entry is the file's entry with the picture's id in place of its position in the file.
        group.extend_from_slice(&ico[entry..entry + 12]);
        u16le(&mut group, id);
    }
    resource(out, RT_GROUP_ICON, 1, 0x1030, LANG_EN_US, &group);
    Ok(())
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
}

/// A node of the version block: length, length of its value, type (1 = text), key, value, children – each part
/// starting on a 4-byte boundary.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![];
    u16le(&mut out, 0); // total length, filled in below
    u16le(&mut out, value_len);
    u16le(&mut out, text as u16);
    out.extend_from_slice(&utf16z(key));
    pad4(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad4(&mut out);
        out.extend_from_slice(child);
    }
    let len = out.len() as u16;
    out[..2].copy_from_slice(&len.to_le_bytes());
    out
}

fn version_numbers(version: &str) -> [u16; 4] {
    let mut out = [0u16; 4];
    let plain = version.split(['-', '+']).next().unwrap_or("");
    for (slot, part) in out.iter_mut().zip(plain.split('.')) {
        *slot = part.parse().unwrap_or(0);
    }
    out
}

fn version_info(v: &Version) -> Vec<u8> {
    let n = version_numbers(v.version);
    let (ms, ls) = ((n[0] as u32) << 16 | n[1] as u32, (n[2] as u32) << 16 | n[3] as u32);
    let mut fixed = vec![];
    for value in [0xFEEF04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3f, 0, 0x0004_0004, 1, 0, 0, 0] {
        u32le(&mut fixed, value);
    }
    let string = |key: &str, value: &str| {
        let data = utf16z(value);
        node(key, &data, (data.len() / 2) as u16, true, &[])
    };
    let strings = [
        string("CompanyName", v.company),
        string("FileDescription", v.description),
        string("FileVersion", v.version),
        string("InternalName", v.file_name),
        string("OriginalFilename", v.file_name),
        string("ProductName", v.product),
        string("ProductVersion", v.version),
    ];
    // 0409 = English (US), 04B0 = Unicode: the language the texts above are labelled with.
    let table = node("040904B0", &[], 0, true, &strings);
    let string_info = node("StringFileInfo", &[], 0, true, &[table]);
    let mut translation = vec![];
    u16le(&mut translation, LANG_EN_US);
    u16le(&mut translation, 0x04B0);
    let var_info = node("VarFileInfo", &[], 0, true, &[node("Translation", &translation, 4, false, &[])]);
    node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_info, var_info])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walks the file the way the linker does: header, data, padding – and returns (type, name, data length).
    fn entries(res: &[u8]) -> Vec<(u16, u16, usize)> {
        let mut out = vec![];
        let mut at = 0;
        while at < res.len() {
            let data = u32::from_le_bytes(res[at..at + 4].try_into().unwrap()) as usize;
            let header = u32::from_le_bytes(res[at + 4..at + 8].try_into().unwrap()) as usize;
            assert_eq!(header, 32);
            let kind = u16::from_le_bytes(res[at + 10..at + 12].try_into().unwrap());
            let name = u16::from_le_bytes(res[at + 14..at + 16].try_into().unwrap());
            out.push((kind, name, data));
            at += header + data.div_ceil(4) * 4;
        }
        assert_eq!(at, res.len());
        out
    }

    fn version() -> Version<'static> {
        Version { description: "Hardware Monitor", product: "Hardware Monitor", company: "Windows Toolbox", file_name: "HardwareMonitor.exe", version: "2.1.0" }
    }

    #[test]
    fn layout() {
        // An .ico with two pictures of 5 and 3 bytes.
        let mut ico = vec![0, 0, 1, 0, 2, 0];
        ico.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0, 5, 0, 0, 0, 38, 0, 0, 0]);
        ico.extend_from_slice(&[32, 32, 0, 0, 1, 0, 32, 0, 3, 0, 0, 0, 43, 0, 0, 0]);
        ico.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let res = build(Some(&ico), &version()).unwrap();
        let e = entries(&res);
        assert_eq!(&e[..4], &[(0, 0, 0), (RT_ICON, 1, 5), (RT_ICON, 2, 3), (RT_GROUP_ICON, 1, 6 + 2 * 14)]);
        assert_eq!((e[4].0, e[5].0), (RT_VERSION, RT_MANIFEST));
        assert!(build(Some(&[1, 2, 3]), &version()).is_err());
        assert!(build(Some(&ico[..30]), &version()).is_err());
    }

    #[test]
    fn version_block() {
        assert_eq!(version_numbers("2.1.0"), [2, 1, 0, 0]);
        assert_eq!(version_numbers("0.3.7-beta.1"), [0, 3, 7, 0]);
        let block = version_info(&version());
        assert_eq!(u16::from_le_bytes([block[0], block[1]]) as usize, block.len());
        assert_eq!(u16::from_le_bytes([block[2], block[3]]), 52);
        // The fixed part starts on a 4-byte boundary after the key and begins with its signature.
        let key_end = 6 + utf16z("VS_VERSION_INFO").len();
        let fixed = key_end.div_ceil(4) * 4;
        assert_eq!(block[fixed..fixed + 4], 0xFEEF04BDu32.to_le_bytes());
        assert_eq!(block[fixed + 8..fixed + 12], (2u32 << 16 | 1).to_le_bytes());
    }
}
