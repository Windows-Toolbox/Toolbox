# Example

The template of a **kit program** for the Windows Toolbox: a small program that runs in the background, has a
page in the Toolbox, a line and a menu entry in the tray, and runs without the Toolbox as well. It counts the
minutes it runs – replace that with what your program does.

## Making a new program from it

1. Copy this folder into an empty folder next to the Toolbox's (`E:\Software\Windows Toolbox\<Name>\`) and make it
   a git repo of its own.
2. Change the names marked with ◆ in `Cargo.toml`, `$package` and `$exe` in `build.ps1`, and this README.
3. Run `build.ps1`. It builds and installs into `dist\` (`dist\<Exe>.exe`, `dist\program.json`); your settings are
   in `dist\data\`, which no build touches.
4. In the Toolbox: **Add program…** → this folder. Start, Stop, the page and "Start with Windows" work at once.
5. Write your program: settings in `Settings`, one line per setting in `page`, the work in `work`. Texts go into
   `assets\i18n.json` (German and English); the tests check that every id there is used and has both languages.
6. Optional: `assets\icon.ico` – the exe, the tray and the Toolbox show it.

What a program may use from the kit and what the page can show stand in the Toolbox's `DESIGN.md` (15.2 the
building blocks of a page, 15.7 the kit's functions).

## A release

`release.ps1` builds, packs `dist\` without `data\` into a zip, signs it with the release key (the `sign` tool
in the Toolbox's repo, which must lie next to this one) and publishes it on GitHub. Installed copies find it with
**Update** in the Toolbox. To appear under **Get programs**, the program gets an entry in the Toolbox's
`programs.json` (signed again with `cargo run -p sign -- file programs.json`).

## Building

Requirement: [Rust](https://rustup.rs). `build.ps1` – nothing pops up; the first build takes a few minutes. The
kit comes from the Toolbox's repo on GitHub (`main`).

## Tests

`cargo test` – the page names only settings that exist, every text exists in every language, status and tray say
what they should.
