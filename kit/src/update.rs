//! Updates (DESIGN.md 9, 15.6) – one way for every kind of program and for the Toolbox itself: look for a
//! release, download its zip, check the signature, swap the files. The same code installs a program from
//! "Get programs".
//!
//! What protects an install, in the order it happens:
//! 1. **The signature** over the zip's exact bytes must fit the key of the version that is already installed (or,
//!    for a first install, the key from the signed list of programs). Nothing of the zip is looked at before.
//! 2. **The manifest inside the zip** – signed along with everything else – must name the expected program and,
//!    for an update, a newer version. So a signed zip of another program, or an old signed release offered again
//!    as "the latest", is refused: the release's name and tag on GitHub are not trusted for anything.
//! 3. **File names** must stay inside the program's folder and outside `data\`, which is the user's.
//! 4. Everything is unpacked aside first; only then files are swapped, and a failure half-way puts the old ones back.

use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::http;
use crate::manifest::Manifest;

/// The key the list of "Get programs" is checked against, and the `update_key` of every kit program.
pub use crate::UPDATE_KEY as KEY;

/// The largest zip that is downloaded, and the largest file in one.
const MAX_ZIP: usize = 300 * 1024 * 1024;
const MAX_SMALL: usize = 1024 * 1024;

/// Where an update is unpacked before files are swapped; never part of a release.
const STAGING: &str = ".update";
/// What `install` remembers in `data\`: the files it wrote, so the next update can remove those that are gone.
const INSTALLED: &str = "installed.json";

// ---------- versions ----------

/// Compares `1.10` and `1.9.2` number by number; missing parts count as 0, anything that is no number as 0.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parts = |v: &str| -> Vec<u64> { v.trim().trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(a), parts(b));
    (0..a.len().max(b.len())).map(|i| a.get(i).copied().unwrap_or(0).cmp(&b.get(i).copied().unwrap_or(0))).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

// ---------- base64 ----------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= chunk.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// `None` if the text is no base64. Line breaks and spaces around it are ignored (a `.sig` file ends with one).
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim().trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        acc = acc << 6 | B64.iter().position(|&b| b == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---------- the signature ----------

/// Checks the signature (base64 of 64 bytes) over `data` against the public key (base64 of 32 bytes).
pub fn verify(data: &[u8], signature: &str, key: &str) -> Result<(), String> {
    let key: [u8; 32] = base64_decode(key).and_then(|k| k.try_into().ok()).ok_or("the update key is not a key")?;
    let signature: [u8; 64] = base64_decode(signature).and_then(|s| s.try_into().ok()).ok_or("the signature file is not a signature")?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| "the update key is not a key")?;
    // "Strict": also refuses the few odd keys and signatures for which more than one message would fit.
    key.verify_strict(data, &Signature::from_bytes(&signature)).map_err(|_| "the signature does not fit – the download is not what was released".to_string())
}

// ---------- zip ----------

/// The small part of the zip format our releases use: files, stored or deflated, names in UTF-8. Written by the
/// `sign` tool of this repo (always the same bytes for the same files), read here – and only after the signature
/// was checked, so the reader never sees bytes somebody else made.
pub mod zip {
    use miniz_oxide::deflate::compress_to_vec;
    use miniz_oxide::inflate::decompress_to_vec_with_limit;

    fn crc32(data: &[u8]) -> u32 {
        static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
        let table = TABLE.get_or_init(|| {
            let mut t = [0u32; 256];
            for (i, slot) in t.iter_mut().enumerate() {
                let mut c = i as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                }
                *slot = c;
            }
            t
        });
        !data.iter().fold(!0u32, |c, &b| table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8))
    }

    fn u16le(v: u16) -> [u8; 2] {
        v.to_le_bytes()
    }
    fn u32le(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// Packs `files` (name with `/`, content) into a zip. No dates, no attributes: the same files give the same
    /// bytes.
    pub fn write(files: &[(String, Vec<u8>)]) -> Vec<u8> {
        // 1980-01-01, the earliest date the format knows.
        const DATE: u16 = 0x0021;
        const UTF8: u16 = 0x0800;
        let (mut out, mut directory) = (Vec::new(), Vec::new());
        for (name, content) in files {
            let packed = compress_to_vec(content, 9);
            let (method, data): (u16, &[u8]) = if packed.len() < content.len() { (8, &packed) } else { (0, content) };
            let offset = out.len() as u32;
            let crc = crc32(content);
            // What the local header and the directory entry share.
            let mut common = Vec::new();
            for part in [&u16le(20)[..], &u16le(UTF8), &u16le(method), &u16le(0), &u16le(DATE), &u32le(crc), &u32le(data.len() as u32), &u32le(content.len() as u32), &u16le(name.len() as u16), &u16le(0)] {
                common.extend_from_slice(part);
            }
            out.extend_from_slice(&u32le(0x0403_4b50));
            out.extend_from_slice(&common);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            directory.extend_from_slice(&u32le(0x0201_4b50));
            directory.extend_from_slice(&u16le(20));
            directory.extend_from_slice(&common);
            // comment length, disk, internal attributes (2 bytes each), external attributes (4)
            directory.extend_from_slice(&[0u8; 10]);
            directory.extend_from_slice(&u32le(offset));
            directory.extend_from_slice(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&u32le(0x0605_4b50));
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&u16le(files.len() as u16));
        out.extend_from_slice(&u16le(files.len() as u16));
        out.extend_from_slice(&u32le(directory.len() as u32));
        out.extend_from_slice(&u32le(start));
        out.extend_from_slice(&u16le(0));
        out
    }

    /// Unpacks a zip: every file with its name (`/` between folders). Folders themselves are left out.
    /// `max_file`: no file may unpack to more than this.
    pub fn read(zip: &[u8], max_file: usize) -> Result<Vec<(String, Vec<u8>)>, String> {
        let short = || "the zip file is cut off".to_string();
        let u16at = |at: usize| zip.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize).ok_or_else(short);
        let u32at = |at: usize| zip.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or_else(short);
        // The end record is the last thing in the file, followed at most by a comment.
        let end = (0..=zip.len().saturating_sub(22)).rev().take(66_000).find(|&i| zip[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06])).ok_or("this is no zip file")?;
        let (count, mut at) = (u16at(end + 10)?, u32at(end + 16)?);
        let mut files = Vec::with_capacity(count);
        for _ in 0..count {
            if u32at(at)? != 0x0201_4b50 {
                return Err("the zip file's list of contents is damaged".into());
            }
            let (flags, method, crc, packed, size) = (u16at(at + 8)?, u16at(at + 10)?, u32at(at + 16)?, u32at(at + 20)?, u32at(at + 24)?);
            let (name_len, extra, comment, local) = (u16at(at + 28)?, u16at(at + 30)?, u16at(at + 32)?, u32at(at + 42)?);
            let name = String::from_utf8(zip.get(at + 46..at + 46 + name_len).ok_or_else(short)?.to_vec()).map_err(|_| "a file name in the zip is not UTF-8")?.replace('\\', "/");
            at += 46 + name_len + extra + comment;
            if name.ends_with('/') {
                continue;
            }
            if flags & 1 != 0 {
                return Err(format!("{name}: encrypted"));
            }
            if size > max_file {
                return Err(format!("{name}: larger than {max_file} bytes"));
            }
            // The local header repeats name and extra field, with lengths of its own.
            let data_at = local + 30 + u16at(local + 26)? + u16at(local + 28)?;
            let data = zip.get(data_at..data_at + packed).ok_or_else(short)?;
            let content = match method {
                0 => data.to_vec(),
                8 => decompress_to_vec_with_limit(data, size).map_err(|_| format!("{name}: cannot be unpacked"))?,
                other => return Err(format!("{name}: packed in a way this program does not know ({other})")),
            };
            if content.len() != size || crc32(&content) as usize != crc {
                return Err(format!("{name}: damaged"));
            }
            files.push((name, content));
        }
        Ok(files)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn what_is_written_is_read() {
            assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
            let files = vec![("a.txt".to_string(), b"hello hello hello hello hello".to_vec()), ("sub/ä.bin".to_string(), vec![7u8, 1, 250]), ("empty".to_string(), vec![])];
            let zip = write(&files);
            assert_eq!(read(&zip, 1000).unwrap(), files);
            assert_eq!(write(&files), zip, "the same files give the same bytes");
            assert!(read(&zip, 10).is_err(), "a file larger than allowed");
            assert!(read(&zip[..zip.len() - 30], 1000).is_err());
            let mut flipped = zip.clone();
            flipped[35] ^= 1;
            assert!(read(&flipped, 1000).is_err(), "a changed byte in a file is noticed");
            assert!(read(b"not a zip", 1000).is_err());
        }
    }
}

// ---------- looking for a release ----------

/// The latest release on GitHub, and where its two files are.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// From the release's tag (`v1.2` → `1.2`). Only for showing and for deciding whether to download – what is
    /// installed is decided by the manifest inside the signed zip.
    pub version: String,
    /// `<id>-<version>.zip` and its `.sig` – `None` if the release lacks it (an old release from before signed
    /// zips: it is still worth knowing its version, which may well be older than the installed one).
    pub zip: Option<String>,
    pub signature: Option<String>,
}

fn is_repository(repository: &str) -> bool {
    // Not `.` or `..`: the name goes into an address.
    let part = |p: &str| !p.is_empty() && !p.starts_with('.') && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    repository.split_once('/').is_some_and(|(owner, name)| part(owner) && part(name))
}

/// What GitHub answers for "the latest release" → its version and, if it has them, `<id>-<version>.zip` and
/// its `.sig`.
pub fn parse_release(json: &str, id: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("GitHub's answer cannot be read: {e}"))?;
    let version = v["tag_name"].as_str().ok_or("GitHub's answer names no release")?.trim_start_matches('v').to_string();
    let asset = |name: String| v["assets"].as_array().into_iter().flatten().find(|a| a["name"] == name.as_str()).and_then(|a| a["browser_download_url"].as_str()).map(str::to_string);
    Ok(Release { zip: asset(format!("{id}-{version}.zip")), signature: asset(format!("{id}-{version}.zip.sig")), version })
}

/// The latest release of `repository` (`owner/name`) – `None` if there is none yet. Asks GitHub; called only
/// after a click (or when the user switched automatic checking on).
pub fn latest(repository: &str, id: &str) -> Result<Option<Release>, String> {
    if !is_repository(repository) {
        return Err(format!("\"{repository}\" is no repository (owner/name)"));
    }
    let url = format!("https://api.github.com/repos/{repository}/releases/latest");
    let answer = http::get(&url, &[("Accept", "application/vnd.github+json")], MAX_SMALL).map_err(|e| format!("GitHub cannot be reached: {e}"))?;
    match answer.status {
        200 => parse_release(&answer.text(), id).map(Some),
        404 => Ok(None),
        other => Err(format!("GitHub answered {other}")),
    }
}

/// Downloads a release and checks its signature against `key`. What comes back are the zip's bytes, as released.
pub fn download(release: &Release, key: &str) -> Result<Vec<u8>, String> {
    let fetch = |url: &str, limit| match http::get(url, &[], limit) {
        Ok(answer) if answer.ok() => Ok(answer.body),
        Ok(answer) => Err(format!("{url}: the server answered {}", answer.status)),
        Err(e) => Err(format!("{url}: {e}")),
    };
    let (Some(zip_url), Some(signature_url)) = (&release.zip, &release.signature) else {
        return Err(format!("the release {} has no signed zip for this program", release.version));
    };
    let signature = fetch(signature_url, MAX_SMALL)?;
    let zip = fetch(zip_url, MAX_ZIP)?;
    verify(&zip, &String::from_utf8_lossy(&signature), key)?;
    Ok(zip)
}

// ---------- installing ----------

/// A copy the developer builds himself (it lies in a `dist\` below its sources, or is a git repo): there the
/// update button gives way to the hint "run build.ps1" (section 9).
pub fn developer_copy(home: &Path) -> bool {
    let sources = |dir: &Path| dir.join(".git").exists() || dir.join("Cargo.toml").exists();
    sources(home) || (home.file_name().is_some_and(|n| n.eq_ignore_ascii_case("dist")) && home.parent().is_some_and(sources))
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Installed {
    version: String,
    files: Vec<String>,
}

/// A name from the zip → where it goes below the program's folder. Refused: anything that would leave the folder,
/// and anything in `data\` – that folder is the user's and no release ever touches it.
fn place(name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    let refused = || format!("the zip holds a file that must not be there: {name}");
    // A colon would name a drive or a hidden second stream of a file.
    if name.is_empty() || name.contains(':') || !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(refused());
    }
    let first = path.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).unwrap_or_default();
    if first == "data" || first == STAGING || is_leftover(path) {
        return Err(refused());
    }
    Ok(path.to_path_buf())
}

/// `Toolbox.153012.old.exe`, `i18n.153012.old`: a file an update or a build put aside because it was in use.
fn is_leftover(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.ends_with(".old") || name.ends_with(".old.exe")
}

/// The name a file in use is put aside under – it keeps running under that name (Windows lets a running exe be
/// renamed, not overwritten).
fn aside(target: &Path, stamp: &str) -> PathBuf {
    let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    match target.extension().filter(|e| e.eq_ignore_ascii_case("exe")) {
        Some(_) => target.with_file_name(format!("{stem}.{stamp}.old.exe")),
        None => target.with_file_name(format!("{}.{stamp}.old", target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
    }
}

/// Installs the zip into `folder`. Call only with bytes whose signature was checked (`download`).
///
/// `id`: the program that is expected. `newer_than`: the installed version – the zip must hold a newer one; `None`
/// for a first install. Programs that run keep running from the files put aside; the caller restarts them.
/// Returns the manifest that is installed now.
pub fn install(zip_bytes: &[u8], folder: &Path, id: &str, newer_than: Option<&str>) -> Result<Manifest, String> {
    let files = zip::read(zip_bytes, MAX_ZIP)?;
    let mut placed = Vec::with_capacity(files.len());
    for (name, content) in &files {
        placed.push((place(name)?, content));
    }
    let manifest = files.iter().find(|(name, _)| name == "program.json").ok_or("the zip holds no program.json")?;
    let manifest = Manifest::parse(&String::from_utf8_lossy(&manifest.1))?;
    if manifest.id != id {
        return Err(format!("the zip holds the program \"{}\", not \"{id}\"", manifest.id));
    }
    if let Some(installed) = newer_than {
        if compare_versions(&manifest.version, installed) != Ordering::Greater {
            return Err(format!("the zip holds version {}, which is not newer than the installed {installed}", manifest.version));
        }
    }

    let io = |what: &Path, e: std::io::Error| format!("{}: {e}", what.display());
    // 1. Everything is written aside. A full disk or a folder without write access shows here, before a single
    //    file of the installed program was touched.
    let staging = folder.join(STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    for (path, content) in &placed {
        let file = staging.join(path);
        std::fs::create_dir_all(file.parent().unwrap_or(&staging)).map_err(|e| io(&file, e))?;
        std::fs::write(&file, content).map_err(|e| io(&file, e))?;
    }

    // 2. The swap: each old file is put aside, the new one takes its place. Renaming within one folder either
    //    happens or does not – and if one step fails, the steps before it are undone.
    let stamp = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = vec![];
    let swapped = (|| {
        for (path, _) in &placed {
            let target = folder.join(path);
            std::fs::create_dir_all(target.parent().unwrap_or(folder)).map_err(|e| io(&target, e))?;
            let old = if target.exists() {
                let old = aside(&target, &stamp);
                std::fs::rename(&target, &old).map_err(|e| io(&target, e))?;
                Some(old)
            } else {
                None
            };
            done.push((target.clone(), old));
            std::fs::rename(staging.join(path), &target).map_err(|e| io(&target, e))?;
        }
        Ok::<(), String>(())
    })();
    if let Err(e) = swapped {
        for (target, old) in done.iter().rev() {
            let _ = std::fs::remove_file(target);
            if let Some(old) = old {
                let _ = std::fs::rename(old, target);
            }
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // 3. Tidy up. Files of the version before that this one no longer has are removed – only files an earlier
    //    install wrote itself, never anything else that lies in the folder.
    let record = folder.join("data").join(INSTALLED);
    let before: Installed = std::fs::read_to_string(&record).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let now: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
    for name in before.files.iter().filter(|name| !now.iter().any(|n| n.eq_ignore_ascii_case(name))) {
        if let Ok(path) = place(name) {
            let target = folder.join(path);
            if std::fs::remove_file(&target).is_err() && target.exists() {
                let _ = std::fs::rename(&target, aside(&target, &stamp));
            }
        }
    }
    let _ = std::fs::create_dir_all(folder.join("data"));
    let _ = std::fs::write(&record, serde_json::to_string_pretty(&Installed { version: manifest.version.clone(), files: now }).unwrap_or_default());
    let _ = std::fs::remove_dir_all(&staging);
    clean_up(folder);
    Ok(manifest)
}

/// Removes what an update or a build put aside and what is no longer in use (a file that still runs cannot be
/// deleted and simply stays until the next time). Cheap: one look at the folder; every program does it at its start.
pub fn clean_up(folder: &Path) {
    let _ = std::fs::remove_dir_all(folder.join(STAGING));
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if is_leftover(&path) && path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kit-update-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn release(version: &str, more: &[(&str, &str)]) -> Vec<u8> {
        let manifest = format!(r#"{{"id":"demo","name":"Demo","version":"{version}","exe":"Demo.exe"}}"#);
        let mut files = vec![("program.json".to_string(), manifest.into_bytes())];
        files.extend(more.iter().map(|(name, content)| (name.to_string(), content.as_bytes().to_vec())));
        zip::write(&files)
    }

    #[test]
    fn versions() {
        for (a, b, expected) in [("1.10", "1.9.2", Ordering::Greater), ("2.0", "2.0.0", Ordering::Equal), ("v1.2", "1.3", Ordering::Less), ("1.0.1", "1.0", Ordering::Greater), ("", "0", Ordering::Equal)] {
            assert_eq!(compare_versions(a, b), expected, "{a} vs {b}");
        }
    }

    #[test]
    fn base64_both_ways() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0u8, 255, 254, 3, 9]] {
            assert_eq!(base64_decode(&base64_encode(data)).as_deref(), Some(data));
        }
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_decode("Zm9vYg==\r\n").as_deref(), Some(&b"foob"[..]));
        assert_eq!(base64_decode("not base64!"), None);
    }

    #[test]
    fn only_what_was_signed_passes() {
        let secret = SigningKey::from_bytes(&[7u8; 32]);
        let key = base64_encode(&secret.verifying_key().to_bytes());
        let zip = release("1.1", &[]);
        let signature = base64_encode(&secret.sign(&zip).to_bytes());
        verify(&zip, &signature, &key).unwrap();
        // A signature file as it is downloaded: with a line break.
        verify(&zip, &format!("{signature}\n"), &key).unwrap();
        let mut changed = zip.clone();
        changed[40] ^= 1;
        assert!(verify(&changed, &signature, &key).is_err(), "one changed bit");
        let other = base64_encode(&SigningKey::from_bytes(&[8u8; 32]).verifying_key().to_bytes());
        assert!(verify(&zip, &signature, &other).is_err(), "another key");
        assert!(verify(&zip, "AAAA", &key).is_err());
        assert!(verify(&zip, &signature, "AAAA").is_err());
        assert!(base64_decode(KEY).is_some_and(|k| k.len() == 32), "the compiled-in key is a key");
    }

    #[test]
    fn an_update_swaps_files_and_leaves_data_alone() {
        let dir = temp("swap");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/settings.json"), "mine").unwrap();
        std::fs::write(dir.join("notes.txt"), "not part of any release").unwrap();

        // A first install, then an update that changes one file, adds one and no longer has another.
        let m = install(&release("1.0", &[("Demo.exe", "one"), ("assets/old.txt", "old"), ("assets/keep.txt", "keep")]), &dir, "demo", None).unwrap();
        assert_eq!(m.version, "1.0");
        install(&release("1.1", &[("Demo.exe", "two"), ("assets/keep.txt", "keep"), ("assets/new.txt", "new")]), &dir, "demo", Some("1.0")).unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        assert_eq!(read("Demo.exe").as_deref(), Some("two"));
        assert_eq!(read("assets/new.txt").as_deref(), Some("new"));
        assert_eq!(read("assets/old.txt"), None, "a file the new version no longer has is removed");
        assert_eq!(read("data/settings.json").as_deref(), Some("mine"));
        assert_eq!(read("notes.txt").as_deref(), Some("not part of any release"));
        assert!(!dir.join(STAGING).exists());
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| is_leftover(&e.path())).collect();
        assert!(leftovers.is_empty(), "nothing is left lying around: {leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn what_does_not_fit_is_refused_before_anything_changes() {
        let dir = temp("refuse");
        install(&release("1.5", &[("Demo.exe", "installed")]), &dir, "demo", None).unwrap();
        let refused = |zip: Vec<u8>, id: &str| {
            let r = install(&zip, &dir, id, Some("1.5"));
            assert_eq!(std::fs::read_to_string(dir.join("Demo.exe")).unwrap(), "installed");
            assert!(!dir.join(STAGING).exists());
            r.unwrap_err()
        };
        // An old release offered again, the same version, another program's release.
        assert!(refused(release("1.4", &[("Demo.exe", "old")]), "demo").contains("not newer"));
        assert!(refused(release("1.5", &[("Demo.exe", "same")]), "demo").contains("not newer"));
        assert!(refused(release("9.0", &[("Demo.exe", "other")]), "another").contains("not \"another\""));
        // Files that would leave the folder or touch the user's data.
        for name in ["../evil.exe", "data/settings.json", "DATA/x", "C:/Windows/x", "/abs", "a/../../b", "x.exe:stream", ".update/x", "Demo.1.old.exe"] {
            assert!(refused(release("2.0", &[(name, "x")]), "demo").contains("must not be there"), "{name}");
        }
        assert!(refused(zip::write(&[("Demo.exe".into(), b"x".to_vec())]), "demo").contains("no program.json"));
        assert!(refused(b"garbage".to_vec(), "demo").contains("no zip"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_in_use_is_put_aside_and_a_failed_swap_is_undone() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp("in-use");
        install(&release("1.0", &[("Demo.exe", "one"), ("a.txt", "a1"), ("z.txt", "z1")]), &dir, "demo", None).unwrap();

        // A program that really runs from Demo.exe (a copy of Windows' own ping, waiting half a minute, without a
        // window): Windows lets such a file be renamed, but not overwritten or deleted.
        use std::os::windows::process::CommandExt;
        let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("ping.exe");
        std::fs::copy(&ping, dir.join("Demo.exe")).unwrap();
        let mut running = std::process::Command::new(dir.join("Demo.exe")).args(["-n", "30", "127.0.0.1"]).creation_flags(0x0800_0000).stdout(std::process::Stdio::null()).spawn().unwrap();
        assert!(std::fs::write(dir.join("Demo.exe"), "x").is_err(), "a running exe cannot be overwritten");
        install(&release("1.1", &[("Demo.exe", "two"), ("a.txt", "a2"), ("z.txt", "z2")]), &dir, "demo", Some("1.0")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("Demo.exe")).unwrap(), "two");
        let old: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| is_leftover(p)).collect();
        assert_eq!(old.len(), 1, "the version in use stays, under another name: {old:?}");
        assert!(old[0].to_string_lossy().ends_with(".old.exe"));
        assert!(running.try_wait().unwrap().is_none(), "and keeps running");
        running.kill().unwrap();
        running.wait().unwrap();
        clean_up(&dir);
        assert!(!old[0].exists(), "once it no longer runs, it is removed");

        // A file that can neither be renamed nor replaced: the update fails, and what it had already swapped is back.
        let locked = std::fs::OpenOptions::new().read(true).share_mode(1).open(dir.join("z.txt")).unwrap();
        assert!(install(&release("1.2", &[("Demo.exe", "three"), ("a.txt", "a3"), ("z.txt", "z3")]), &dir, "demo", Some("1.1")).is_err());
        drop(locked);
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!((read("Demo.exe").as_str(), read("a.txt").as_str(), read("z.txt").as_str()), ("two", "a2", "z2"));
        let manifest: serde_json::Value = serde_json::from_str(&read("program.json")).unwrap();
        assert_eq!(manifest["version"], "1.1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn github_s_answer() {
        let json = r#"{"tag_name":"v1.2","assets":[
            {"name":"demo-1.2.zip","browser_download_url":"https://github.com/o/r/releases/download/v1.2/demo-1.2.zip"},
            {"name":"demo-1.2.zip.sig","browser_download_url":"https://github.com/o/r/releases/download/v1.2/demo-1.2.zip.sig"}]}"#;
        let r = parse_release(json, "demo").unwrap();
        assert_eq!((r.version.as_str(), r.zip.unwrap().ends_with("demo-1.2.zip"), r.signature.unwrap().ends_with(".sig")), ("1.2", true, true));
        // A release without our files (another program's, or one from before signed zips): its version still
        // counts, the download says why it cannot be done.
        let other = parse_release(json, "other").unwrap();
        assert_eq!((other.version.as_str(), other.zip.is_none()), ("1.2", true));
        assert!(download(&other, KEY).unwrap_err().contains("no signed zip"));
        assert!(parse_release("{}", "demo").is_err());
        assert!(is_repository("Windows-Toolbox/Discord-Plus"));
        for bad in ["nothing", "a/b/c", "a/", "../x", "a/b?x", "a b/c"] {
            assert!(!is_repository(bad), "{bad}");
        }
    }

    #[test]
    fn a_copy_that_is_built_here_is_not_updated() {
        let dir = temp("dev");
        std::fs::create_dir_all(dir.join("Repo/dist")).unwrap();
        std::fs::create_dir_all(dir.join("Unpacked/dist")).unwrap();
        std::fs::create_dir_all(dir.join("Script/.git")).unwrap();
        std::fs::write(dir.join("Repo/Cargo.toml"), "").unwrap();
        assert!(developer_copy(&dir.join("Repo/dist")));
        assert!(developer_copy(&dir.join("Script")));
        assert!(!developer_copy(&dir.join("Unpacked/dist")));
        assert!(!developer_copy(&dir.join("Unpacked")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
