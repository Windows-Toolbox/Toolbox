//! toolbox-kit: what the Toolbox and the programs share (DESIGN.md 5). Compiled into every exe – nothing is looked
//! up at runtime, so a program runs without a Toolbox and with whatever kit version it was built with.

#[cfg(feature = "build")]
pub mod build;
#[cfg(feature = "build")]
mod res;

#[cfg(feature = "runtime")]
pub mod app;
#[cfg(feature = "runtime")]
pub mod i18n;
#[cfg(feature = "runtime")]
pub mod ipc;
#[cfg(feature = "runtime")]
pub mod manifest;
#[cfg(feature = "runtime")]
pub mod page;
#[cfg(feature = "runtime")]
pub mod process;
#[cfg(feature = "runtime")]
pub mod proto;
#[cfg(feature = "runtime")]
pub mod settings;
#[cfg(feature = "runtime")]
pub mod tray;
#[cfg(feature = "runtime")]
pub mod ui;

/// The newest protocol this kit speaks (DESIGN.md 4.3). Both sides of a pipe use the smaller of their two numbers.
pub const PROTOCOL: u32 = 1;

/// Facts about the program that exist once, in its `Cargo.toml`, and reach the code through its build script
/// (`kit::build::manifest()`): `const META: kit::Meta = kit::meta!();`
#[derive(Debug, Clone, Copy)]
pub struct Meta {
    /// `[package.metadata.toolbox] id` – pipe, task, autostart and job names derive from it.
    pub id: &'static str,
    /// `[package] version`
    pub version: &'static str,
    /// The program's `dist\` folder on the PC it was built on. Only used when the exe is started from cargo's
    /// build folder (`cargo run`): it then uses the same `dist\data\` as the built program (DESIGN.md 3).
    pub dist: &'static str,
    /// The generated `program.json` (DESIGN.md 4.1).
    pub manifest: &'static str,
}

/// The program's [`Meta`]. Needs `kit::build::manifest()` in the program's `build.rs`.
#[macro_export]
macro_rules! meta {
    () => {
        $crate::Meta {
            id: env!("TOOLBOX_ID"),
            version: env!("CARGO_PKG_VERSION"),
            dist: env!("TOOLBOX_DIST"),
            manifest: env!("TOOLBOX_MANIFEST"),
        }
    };
}
