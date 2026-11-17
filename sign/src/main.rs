//! The developer's tool behind every `release.ps1` (DESIGN.md 9, 15.6). Not part of any release.
//!
//!   sign key                 shows the public key – and creates the private one the first time
//!   sign pack <dist> <zip>   packs the folder (without data\) into the zip and writes <zip>.sig next to it
//!   sign file <file>         writes <file>.sig (for programs.json)
//!
//! The private key is a 32-byte seed, base64, in `%USERPROFILE%\.windows-toolbox\release-key` – outside every
//! repo. Whoever has it can release updates that every installed program accepts: keep it safe and backed up.
//! If it is lost, the next version has to be downloaded by hand once.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ed25519_dalek::{Signer, SigningKey};
use kit::update::{base64_decode, base64_encode, zip};

/// Internal, for tests: another place for the key file.
const KEY_FILE_VARIABLE: &str = "TOOLBOX_RELEASE_KEY";

fn key_file() -> Result<PathBuf, String> {
    if let Some(file) = std::env::var_os(KEY_FILE_VARIABLE) {
        return Ok(file.into());
    }
    let home = std::env::var_os("USERPROFILE").ok_or("USERPROFILE is not set")?;
    Ok(PathBuf::from(home).join(".windows-toolbox").join("release-key"))
}

/// The private key – created once, never replaced: a file that is there but unreadable is an error, not a reason
/// to make a new key (every installed program would refuse the next update).
fn key() -> Result<SigningKey, String> {
    let file = key_file()?;
    match std::fs::read_to_string(&file) {
        Ok(text) => {
            let seed: [u8; 32] = base64_decode(&text).and_then(|s| s.try_into().ok()).ok_or(format!("{} does not hold a key", file.display()))?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| format!("no random numbers: {e}"))?;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            // `create_new`: never over an existing file, whatever happened in between.
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            writeln!(f, "{}", base64_encode(&seed)).map_err(|e| format!("{}: {e}", file.display()))?;
            eprintln!("A new release key was created: {}\nBack this file up – without it no update can be released.", file.display());
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// Every file below `dir`, name with `/`, sorted – so the same folder always gives the same zip.
fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        // Not part of a release: the user's data, what an update or a build put aside, and an update in progress.
        if (prefix.is_empty() && (lower == "data" || lower == ".update")) || lower.ends_with(".old") || lower.ends_with(".old.exe") {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect(&path, &format!("{prefix}{name}/"), out)?;
        } else {
            out.push((format!("{prefix}{name}"), std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?));
        }
    }
    Ok(())
}

fn sign_file(file: &Path, data: &[u8]) -> Result<(), String> {
    let signature = base64_encode(&key()?.sign(data).to_bytes());
    let mut sig = file.as_os_str().to_owned();
    sig.push(".sig");
    std::fs::write(&sig, format!("{signature}\n")).map_err(|e| format!("{}: {e}", Path::new(&sig).display()))
}

fn run(args: &[String]) -> Result<(), String> {
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["key"] => println!("{}", base64_encode(&key()?.verifying_key().to_bytes())),
        ["pack", dist, zip_file] => {
            let mut files = vec![];
            collect(Path::new(dist), "", &mut files)?;
            if !files.iter().any(|(name, _)| name == "program.json") {
                return Err(format!("{dist} holds no program.json – run build.ps1 first"));
            }
            let bytes = zip::write(&files);
            // What is released must be what an update will accept.
            if zip::read(&bytes, usize::MAX)? != files {
                return Err("the zip does not read back as it was written".into());
            }
            std::fs::write(zip_file, &bytes).map_err(|e| format!("{zip_file}: {e}"))?;
            sign_file(Path::new(zip_file), &bytes)?;
            println!("{zip_file}: {} files, {} KB", files.len(), bytes.len() / 1024);
        }
        ["file", file] => sign_file(Path::new(file), &std::fs::read(file).map_err(|e| format!("{file}: {e}"))?)?,
        _ => return Err("usage: sign key | sign pack <dist> <zip> | sign file <file>".into()),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sign: {e}");
            ExitCode::FAILURE
        }
    }
}
