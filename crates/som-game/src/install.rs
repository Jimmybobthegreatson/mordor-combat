//! The game reads its animations and data from your own Shadow of Mordor install, so it asks where that is before anything runs.
//! The folder is remembered; `SOM_DIR` overrides it, and `--set-install` asks again.

use std::path::PathBuf;

use som_formats::vfs::Vfs;

/// Make sure `SOM_DIR` points at a valid install, asking with a folder dialog when it is not known yet. Exits when none is chosen.
pub fn ensure() {
    let ask_again = std::env::args().any(|a| a == "--set-install");
    if !ask_again && Vfs::locate().is_ok_and(|dir| Vfs::is_install(&dir)) {
        return;
    }
    loop {
        let picked = rfd::FileDialog::new()
            .set_title("Select your Middle-earth: Shadow of Mordor folder")
            .pick_folder();
        let Some(dir) = picked else {
            eprintln!("No install folder chosen. This project needs your own copy of Middle-earth: Shadow of Mordor; run again and select its folder, or set SOM_DIR.");
            std::process::exit(1);
        };
        // The game folder itself; if its `x64` subfolder was picked, its parent is the folder meant.
        let dir = if Vfs::is_install(&dir) { Some(dir) } else { dir.parent().filter(|p| Vfs::is_install(p)).map(|p| p.to_path_buf()) };
        if let Some(dir) = dir {
            remember(&dir);
            return;
        }
        rfd::MessageDialog::new()
            .set_title("Not a Shadow of Mordor install")
            .set_description("That folder is not a Shadow of Mordor install. Pick your Middle-earth: Shadow of Mordor game folder.")
            .set_level(rfd::MessageLevel::Warning)
            .show();
    }
}

fn remember(dir: &PathBuf) {
    if let Err(err) = Vfs::save_install(dir) {
        eprintln!("could not remember the install folder: {err:#}");
    }
    // Single-threaded: nothing else reads the environment yet.
    unsafe { std::env::set_var("SOM_DIR", dir) };
}
