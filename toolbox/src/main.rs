//! The Windows Toolbox (DESIGN.md 8): one exe, two roles. The **agent** runs all day – tray icon, the list of
//! programs, their status – on a plain Windows message loop, without egui. The **window** is a second process of
//! the same exe that exists only while it is open.
#![windows_subsystem = "windows"]

mod agent;
mod classic;
mod library;
mod talk;
mod window;

pub const META: kit::Meta = kit::meta!();

/// Internal arguments between our own processes (never typed by a user).
/// The agent starts the window: `--window [id]`.
pub const WINDOW: &str = "--window";
/// The agent starts without opening the window (start with Windows; tests).
pub const BACKGROUND: &str = "--background";
/// "Restart the Toolbox": the new agent first waits until the old one (this process id) has ended – otherwise it
/// would find the old one's pipe, take it for a running Toolbox and end at once. `--after <pid>`.
pub const AFTER: &str = "--after";
/// How long the new agent waits for the old one at most.
const AFTER_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

fn main() {
    kit::texts!();
    kit::app::init(&META);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == WINDOW) {
        window::run(args.get(1).cloned());
    } else {
        let after = args.iter().position(|a| a == AFTER).and_then(|i| args.get(i + 1)?.parse::<u32>().ok());
        if let Some(pid) = after {
            kit::process::wait_exit(pid, AFTER_WAIT);
        }
        agent::run(!args.iter().any(|a| a == BACKGROUND));
    }
}

#[cfg(test)]
mod tests {
    /// Every text id used in the code exists, in every language (DESIGN.md 13).
    #[test]
    fn texts_are_complete() {
        kit::i18n::check_ids(env!("CARGO_MANIFEST_DIR")).unwrap();
    }
}
