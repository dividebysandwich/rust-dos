//! GOG's installs of DOS games: their `goggame-*.info` says how GOG's
//! launcher runs DOSBox, with which configuration files, in which
//! directory. Without one, the folder's `dosbox*.conf` files are read.

use super::{Imported, dosbox, host_path};
use serde_json::Value;
use crate::hostfs as fs;
use std::path::{Path, PathBuf};

/// The `goggame-*.info` of the install in `dir`: there, in its `app`
/// folder, or in a folder up to three levels below.
pub fn find_info(dir: &Path) -> Option<PathBuf> {
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for d in &level {
            let Ok(entries) = fs::read_dir(d) else { continue };
            let mut entries: Vec<_> = entries.into_iter().map(|e| e.path).collect();
            entries.sort();
            if let Some(info) = entries.iter().find(|p| is_info(p)) {
                return Some(info.clone());
            }
            next.extend(entries.into_iter().filter(|p| fs::is_dir(p)));
        }
        level = next;
    }
    None
}

fn is_info(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    name.starts_with("goggame-") && name.ends_with(".info") && fs::is_file(path)
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
        .map(|e| e.path)
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
    match base.filter(|b| fs::is_file(b) && b.as_path() != conf) {
        Some(base) => vec![base, conf.to_path_buf()],
        None => vec![conf.to_path_buf()],
    }
}

/// Where DOSBox may have run for a folder's configuration: GOG's DOSBOX
/// folder in it, the folder itself, or one up to four levels above it, as
/// collections such as eXoDOS keep each game's configuration in a folder
/// of its own and run DOSBox from their root.
fn bases(dir: &Path) -> Vec<PathBuf> {
    let mut bases = vec![dir.join("DOSBOX"), dir.to_path_buf()];
    bases.extend(dir.ancestors().skip(1).take(4).map(Path::to_path_buf));
    bases
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
            import_confs(&confs, &bases(dir), &name, &[], home, dir)
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
    let found = tasks
        .iter()
        .filter(runs_dosbox)
        .find(|t| t["isPrimary"].as_bool() == Some(true))
        .or_else(|| tasks.iter().find(runs_dosbox));
    let Some(task) = found else {
        return import_windows(info, install, name, &tasks);
    };
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
    import_confs(&confs, &[working_dir, install.to_path_buf()], &name, &commands, home, install)
}

/// A game GOG starts without DOSBox: the Windows program of its primary
/// task (or its first that runs one), in the built-in Windows, with the
/// install folder as C:. The commands go to the task's working folder,
/// or the program's, and run the program by its DOS names (`short_path`)
/// with the task's arguments.
fn import_windows(
    info: &Path,
    install: &Path,
    name: String,
    tasks: &[Value],
) -> Result<Imported, String> {
    let runs_exe = |task: &&Value| {
        task["type"].as_str().is_none_or(|t| t == "FileTask")
            && task["path"]
                .as_str()
                .is_some_and(|p| p.to_ascii_lowercase().ends_with(".exe"))
    };
    let task = tasks
        .iter()
        .filter(runs_exe)
        .find(|t| t["isPrimary"].as_bool() == Some(true))
        .or_else(|| tasks.iter().find(runs_exe))
        .ok_or_else(|| {
            format!(
                "{}: GOG runs this game with neither DOSBox nor a Windows program",
                info.display()
            )
        })?;
    let program = task["path"].as_str().unwrap_or_default();
    let exe = short_path(install, program).ok_or_else(|| {
        format!(
            "{}: the program {} isn't in the install folder",
            info.display(),
            program
        )
    })?;
    let working = task["workingDir"]
        .as_str()
        .map(str::trim)
        .filter(|w| !w.is_empty() && *w != ".");
    let folder = match working {
        Some(dir) => short_path(install, dir).ok_or_else(|| {
            format!(
                "{}: the working folder {} isn't in the install folder",
                info.display(),
                dir
            )
        })?,
        None => exe
            .rsplit_once('\\')
            .map_or(String::new(), |(dir, _)| dir.to_string()),
    };
    let mut autoexec = vec!["C:".to_string(), format!("CD \\{}", folder)];
    let args = task["arguments"]
        .as_str()
        .map(str::trim)
        .unwrap_or_default();
    autoexec.push(if args.is_empty() {
        format!("\\{}", exe)
    } else {
        format!("\\{} {}", exe, args)
    });
    let c = crate::mount::MountSpec {
        drive: crate::disk::DRIVE_C,
        path: install.to_path_buf(),
        opts: Default::default(),
    };
    Ok(Imported {
        name,
        drives: vec![c],
        autoexec,
        windows: Some("builtin".to_string()),
        ..Imported::default()
    })
}

/// The DOS names of the path `rel` (relative, `\` or `/` between parts)
/// in the host folder `root`, as the built-in DOS shows a host folder's
/// files (`disk::short_names`), joined by `\`; None when a part isn't
/// there.
fn short_path(root: &Path, rel: &str) -> Option<String> {
    let mut dir = root.to_path_buf();
    let mut parts = Vec::new();
    for part in rel
        .split(['\\', '/'])
        .filter(|p| !p.is_empty() && *p != ".")
    {
        let mut names: Vec<String> = fs::read_dir(&dir)
            .ok()?
            .into_iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort_by_key(|n| n.to_ascii_uppercase());
        let short = crate::disk::short_names(&names);
        let at = names.iter().position(|n| n.eq_ignore_ascii_case(part))?;
        dir.push(&names[at]);
        parts.push(short[at].clone());
    }
    Some(parts.join("\\"))
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
    import_confs(&with_base(conf), &bases(dir), &name, &[], home, dir)
}

/// The game's folder `c_root` is C: if the files mount none.
fn import_confs(
    confs: &[PathBuf],
    bases: &[PathBuf],
    name: &str,
    commands: &[String],
    home: Option<&Path>,
    c_root: &Path,
) -> Result<Imported, String> {
    let texts = confs
        .iter()
        .map(|p| fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| format!("{}: {}", p.display(), e)))
        .collect::<Result<Vec<String>, String>>()?;
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut imported = dosbox::import_in(&texts, bases, name, home, Some(c_root));
    imported.autoexec.extend(commands.iter().cloned());
    if imported.autoexec.is_empty() {
        return Err(format!("{}: the configuration starts no program", name));
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn a_gog_windows_game_runs_in_the_built_in_windows() {
        let dir = scratch("windows");
        fs::create_dir_all(dir.join("System Files")).unwrap();
        fs::write(dir.join("System Files/Myth II.exe"), "MZ").unwrap();
        fs::write(
            dir.join("goggame-1207658932.info"),
            r#"{"gameId":"1207658932","name":"Myth II","playTasks":[
            {"category":"document","path":"manual.pdf","type":"FileTask"},
            {"category":"game","isPrimary":true,"path":"System Files\\Myth II.exe",
             "arguments":"-nosound","type":"FileTask","workingDir":""}]}"#,
        )
        .unwrap();
        let imported = import(&dir, None).unwrap();
        assert_eq!(imported.name, "Myth II");
        assert_eq!(imported.windows.as_deref(), Some("builtin"));
        assert_eq!(imported.drives.len(), 1);
        assert_eq!(imported.drives[0].path, dir);
        assert_eq!(
            imported.autoexec,
            ["C:", r"CD \SYSTEM~1", r"\SYSTEM~1\MYTHII~1.EXE -nosound"]
        );
        let text = imported.profile_text(None);
        assert!(
            text.starts_with("[game]\nname=Myth II\noverlay=true\nwindows=builtin\n"),
            "{}",
            text
        );
        let own = crate::config::parse(&text, Path::new("/"), None);
        assert!(own.warnings.is_empty(), "{:?}", own.warnings);
        assert_eq!(own.game_windows.as_deref(), Some("builtin"));
    }

    #[test]
    fn a_gog_windows_game_starts_in_its_working_folder() {
        let dir = scratch("windows-working");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("Data")).unwrap();
        fs::write(dir.join("bin/game.exe"), "MZ").unwrap();
        fs::write(
            dir.join("goggame-1.info"),
            r#"{"name":"Game","playTasks":[
            {"isPrimary":true,"path":"bin/game.exe","type":"FileTask","workingDir":"Data"}]}"#,
        )
        .unwrap();
        let imported = import(&dir, None).unwrap();
        assert_eq!(imported.autoexec, ["C:", r"CD \DATA", r"\BIN\GAME.EXE"]);
        fs::write(
            dir.join("goggame-1.info"),
            r#"{"name":"Game","playTasks":[
            {"isPrimary":true,"path":"missing.exe","type":"FileTask"}]}"#,
        )
        .unwrap();
        assert!(
            import(&dir, None)
                .unwrap_err()
                .contains("missing.exe isn't in the install folder")
        );
        fs::write(
            dir.join("goggame-1.info"),
            r#"{"name":"Game","playTasks":[{"isPrimary":true,"path":"readme.txt","type":"FileTask"}]}"#,
        )
        .unwrap();
        assert!(
            import(&dir, None)
                .unwrap_err()
                .contains("neither DOSBox nor a Windows program")
        );
    }
}
