//! GOG's installs of DOS games: their `goggame-*.info` says how GOG's
//! launcher runs DOSBox, with which configuration files, in which
//! directory. Without one, the folder's `dosbox*.conf` files are read.

use super::{Imported, dosbox, host_path};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// The `goggame-*.info` of the install in `dir`: there, in its `app`
/// folder, or in a folder up to three levels below.
pub fn find_info(dir: &Path) -> Option<PathBuf> {
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for d in &level {
            let Ok(entries) = fs::read_dir(d) else { continue };
            let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            entries.sort();
            if let Some(info) = entries.iter().find(|p| is_info(p)) {
                return Some(info.clone());
            }
            next.extend(entries.into_iter().filter(|p| p.is_dir()));
        }
        level = next;
    }
    None
}

fn is_info(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    name.starts_with("goggame-") && name.ends_with(".info") && path.is_file()
}

/// The DOSBox configuration files of a folder without a GOG info file, in
/// the order DOSBox reads them: GOG's installs start the game with the
/// settings of `dosboxGame.conf` and the commands of
/// `dosboxGame_single.conf` (its other files set up multiplayer or run the
/// game's setup). A folder with a single file has that.
fn conf_files(dir: &Path) -> Vec<PathBuf> {
    let confs: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            name.starts_with("dosbox") && name.ends_with(".conf")
        })
        .collect();
    match confs.iter().find(|p| p.to_string_lossy().to_ascii_lowercase().ends_with("_single.conf")) {
        Some(single) => with_base(single),
        None if confs.len() == 1 => confs,
        None => Vec::new(),
    }
}

/// A configuration file of GOG's that adds commands to another's
/// settings (`dosboxGame_single.conf` to `dosboxGame.conf`): both, the
/// settings first. Any other file alone.
pub fn with_base(conf: &Path) -> Vec<PathBuf> {
    let name = conf.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let base = name.rsplit_once('_').map(|(stem, _)| conf.with_file_name(format!("{}.conf", stem)));
    match base.filter(|b| b.is_file() && b.as_path() != conf) {
        Some(base) => vec![base, conf.to_path_buf()],
        None => vec![conf.to_path_buf()],
    }
}

/// Where DOSBox may have run for a folder's configuration: GOG's DOSBOX
/// folder in it, or the folder itself.
fn bases(dir: &Path) -> Vec<PathBuf> {
    vec![dir.join("DOSBOX"), dir.to_path_buf()]
}

/// Import the game in `dir`: a GOG install, or a folder with DOSBox
/// configuration files.
pub fn import(dir: &Path, home: Option<&Path>) -> Result<Imported, String> {
    match find_info(dir) {
        Some(info) => import_info(&info, home),
        None => {
            let confs = conf_files(dir);
            if confs.is_empty() {
                return Err(format!("{}: no GOG game or DOSBox configuration to start it with", dir.display()));
            }
            let name = dir.file_name().map_or("Game".to_string(), |n| n.to_string_lossy().into_owned());
            import_confs(&confs, &bases(dir), &name, &[], home)
        }
    }
}

/// Import the game GOG's launcher runs as the primary task of `info`.
fn import_info(info: &Path, home: Option<&Path>) -> Result<Imported, String> {
    let text = fs::read_to_string(info).map_err(|e| format!("{}: {}", info.display(), e))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {}", info.display(), e))?;
    let install = info.parent().unwrap_or(Path::new("."));
    let name = json["name"].as_str().unwrap_or("Game").to_string();
    let tasks = json["playTasks"].as_array().cloned().unwrap_or_default();
    let runs_dosbox = |task: &&Value| task["path"].as_str().is_some_and(|p| p.to_ascii_lowercase().ends_with("dosbox.exe"));
    let task = tasks
        .iter()
        .filter(runs_dosbox)
        .find(|t| t["isPrimary"].as_bool() == Some(true))
        .or_else(|| tasks.iter().find(runs_dosbox))
        .ok_or_else(|| format!("{}: GOG runs this game without DOSBox", info.display()))?;
    let working_dir = host_path(install, task["workingDir"].as_str().unwrap_or(""));
    let args = crate::mount::tokenize(task["arguments"].as_str().unwrap_or(""))?;
    let (mut confs, mut commands) = (Vec::new(), Vec::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_ascii_lowercase().as_str() {
            "-conf" => confs.extend(args.next().map(|p| host_path(&working_dir, p))),
            "-c" => commands.extend(args.next().filter(|c| !c.eq_ignore_ascii_case("exit")).cloned()),
            _ => {}
        }
    }
    if confs.is_empty() {
        confs = conf_files(install);
    }
    import_confs(&confs, &[working_dir, install.to_path_buf()], &name, &commands, home)
}

/// Import a DOSBox configuration file (with the settings of the file it
/// adds commands to, see `with_base`), relative to its folder.
pub fn import_conf(conf: &Path, home: Option<&Path>) -> Result<Imported, String> {
    let dir = conf.parent().unwrap_or(Path::new("."));
    let stem = conf.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match stem.to_ascii_lowercase().strip_prefix("dosbox") {
        Some(_) => dir.file_name().map_or(stem.clone(), |n| n.to_string_lossy().into_owned()),
        None => stem,
    };
    import_confs(&with_base(conf), &bases(dir), &name, &[], home)
}

fn import_confs(confs: &[PathBuf], bases: &[PathBuf], name: &str, commands: &[String], home: Option<&Path>) -> Result<Imported, String> {
    let texts = confs
        .iter()
        .map(|p| fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| format!("{}: {}", p.display(), e)))
        .collect::<Result<Vec<String>, String>>()?;
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut imported = dosbox::import(&texts, bases, name, home);
    imported.autoexec.extend(commands.iter().cloned());
    if imported.autoexec.is_empty() {
        return Err(format!("{}: the configuration starts no program", name));
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("target/test_import_gog").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn a_gog_install_is_run_as_its_launcher_runs_it() {
        let dir = scratch("install");
        let game = dir.join("Some Game");
        fs::create_dir_all(game.join("DOSBOX")).unwrap();
        fs::create_dir_all(game.join("cloud_saves")).unwrap();
        fs::write(game.join("goggame-1207658853.info"), r#"{"gameId":"1207658853","name":"Some Game","playTasks":[
            {"category":"document","path":"manual.pdf","type":"FileTask"},
            {"arguments":"-conf \"..\\dosboxGame.conf\" -conf \"..\\dosboxGame_single.conf\" -noconsole -c \"exit\"","isPrimary":true,"path":"DOSBOX\\dosbox.exe","type":"FileTask","workingDir":"DOSBOX"}]}"#).unwrap();
        fs::write(game.join("dosboxGame.conf"), "[cpu]\ncycles=fixed 20000\n").unwrap();
        fs::write(game.join("dosboxGame_single.conf"), "[autoexec]\n@echo off\ncd ..\nmount C \"..\"\nimgmount d \"..\\cloud_saves\\GAME.INS\" -t iso\nc:\ngame\nexit\n").unwrap();
        fs::write(game.join("cloud_saves/game.ins"), "").unwrap();

        assert_eq!(find_info(&dir), Some(game.join("goggame-1207658853.info")));
        let imported = import(&dir, None).unwrap();
        assert_eq!(imported.name, "Some Game");
        assert_eq!(imported.drives[0].path, game);
        assert_eq!(imported.drives[1].path, game.join("cloud_saves/game.ins"));
        assert_eq!(imported.autoexec, ["@echo off", "c:", "game"]);
        assert!(imported.settings.iter().any(|(_, k, v)| *k == "cycles" && v == "20000"));
    }

    #[test]
    fn a_folder_of_dosbox_files_is_read_without_one() {
        // As GOG's older installs have them, the game in the folder DOSBox
        // was one below.
        let dir = scratch("Descent");
        fs::write(dir.join("dosboxDescent_single.conf"), "[autoexec]\nmount C \"..\\Descent\"\nc:\nDescent.bat\nexit\n").unwrap();
        fs::write(dir.join("dosboxDescent_settings.conf"), "[autoexec]\nmount C \"..\\Descent\"\nc:\nsetup.exe\nexit\n").unwrap();
        fs::write(dir.join("dosboxDescent.conf"), "[cpu]\ncycles=50000\n[autoexec]\n").unwrap();
        let imported = import(&dir, None).unwrap();
        assert_eq!(imported.name, "Descent");
        assert_eq!(imported.drives[0].path, dir);
        assert_eq!(imported.autoexec, ["c:", "Descent.bat"]);
        assert!(imported.settings.iter().any(|(_, k, v)| *k == "cycles" && v == "50000"));
        // Its setup, from its own file.
        let setup = import_conf(&dir.join("dosboxDescent_settings.conf"), None).unwrap();
        assert_eq!(setup.autoexec, ["c:", "setup.exe"]);
        assert!(import(&scratch("empty"), None).is_err());
    }
}
