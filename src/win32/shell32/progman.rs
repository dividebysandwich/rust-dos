//! Program Manager's DDE commands, which Windows 95's Explorer answers
//! for the setup programs written for Windows 3.1's Program Manager: a
//! group is a folder in the Start Menu's Programs, an item a shell link
//! in it. DDEML (user32/ddeml.rs) brings the commands here.
//!
//! The commands are those of Program Manager, each in brackets, several
//! to an execute string: CreateGroup(name), ShowGroup(name, command),
//! DeleteGroup(name), AddItem(command line, name, icon, index, x, y,
//! directory, hot key, minimized), DeleteItem(name), ReplaceItem(name),
//! Reload and ExitProgman. AddItem and DeleteItem work in the group the
//! last CreateGroup or ShowGroup named. CreateGroup and ShowGroup open
//! the group's folder window, as Explorer does.

use super::folders;
use super::link::{self, LinkData};
use crate::cpu::Cpu;
use crate::win32::kernel32::{self, files};

/// One command: its name and arguments.
fn parse(text: &str) -> Option<Vec<(String, Vec<String>)>> {
    let mut commands = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        rest = rest.strip_prefix('[')?;
        let open = rest.find('(');
        let close_bracket = rest.find(']')?;
        let (name, args, after) = match open {
            Some(o) if o < close_bracket => {
                let (args, len) = arguments(&rest[o + 1..])?;
                let after = rest[o + 1 + len..].trim_start().strip_prefix(']')?;
                (rest[..o].trim().to_string(), args, after)
            }
            _ => (
                rest[..close_bracket].trim().to_string(),
                Vec::new(),
                &rest[close_bracket + 1..],
            ),
        };
        commands.push((name, args));
        rest = after.trim_start();
    }
    Some(commands)
}

/// The comma-separated arguments up to the closing parenthesis, quotes
/// kept together, and how many bytes they took with it.
fn arguments(text: &str) -> Option<(Vec<String>, usize)> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => args.push(std::mem::take(&mut current).trim().to_string()),
            ')' if !quoted => {
                let last = current.trim().to_string();
                if !last.is_empty() || !args.is_empty() {
                    args.push(last);
                }
                return Some((args, i + 1));
            }
            _ => current.push(c),
        }
    }
    None
}

/// `s` without the quotes around it.
fn unquote(s: &str) -> String {
    let t = s.trim();
    match t.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        Some(inner) => inner.to_string(),
        None => t.to_string(),
    }
}

/// The folder of the group `name`, and the name its folder has.
fn group_folder(cpu: &mut Cpu, name: &str) -> (String, String) {
    let programs = folders::path(cpu, 0x02).unwrap_or_default();
    let found = files::find(cpu, &programs, name)
        .ok()
        .and_then(|e| e.into_iter().find(|e| e.attributes & 0x10 != 0));
    let shown = found.map_or(name.to_string(), |e| e.name);
    (format!("{}\\{}", programs, shown), shown)
}

fn delete_tree(cpu: &mut Cpu, dir: &str) -> bool {
    for entry in files::find(cpu, dir, "*").unwrap_or_default() {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        let path = format!("{}\\{}", dir, entry.name);
        let done = if entry.attributes & 0x10 != 0 {
            delete_tree(cpu, &path)
        } else {
            let _ = files::set_attributes(cpu, &path, 0);
            files::delete_file(cpu, &path).is_ok()
        };
        if !done {
            return false;
        }
    }
    files::remove_directory(cpu, dir).is_ok()
}

/// The program and the arguments of a command line. The program ends
/// where CreateProcessA finds it does, so an unquoted one can have spaces
/// in its path; one that isn't there ends at its closing quote, or at the
/// first space.
fn split_command(cpu: &mut Cpu, line: &str) -> (String, String) {
    let line = line.trim();
    if let Some(found) = kernel32::program_in_line(cpu, line) {
        return (found.name.to_string(), found.args.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        let args = rest.get(end + 1..).unwrap_or("").trim().to_string();
        return (rest[..end].to_string(), args);
    }
    match line.split_once(' ') {
        Some((p, a)) => (p.to_string(), a.trim().to_string()),
        None => (line.to_string(), String::new()),
    }
}

/// AddItem's arguments made a shell link in the group folder `dir`.
fn add_item(cpu: &mut Cpu, dir: &str, args: &[String]) -> bool {
    let Some(line) = args.first().filter(|l| !l.is_empty()) else {
        return false;
    };
    let (program, arguments) = split_command(cpu, line);
    let path = if program.contains(['\\', ':']) {
        files::full_path(cpu, &program)
    } else {
        kernel32::search_file(cpu, &program).unwrap_or(program.clone())
    };
    let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
    let title = match args.get(1).map(|n| unquote(n)).filter(|n| !n.is_empty()) {
        Some(name) => name,
        None => file
            .rsplit_once('.')
            .map_or(file.clone(), |(stem, _)| stem.to_string()),
    };
    let parent = path
        .rsplit_once('\\')
        .map_or(String::new(), |(d, _)| d.to_string());
    let directory = args
        .get(6)
        .map(|d| unquote(d))
        .filter(|d| !d.is_empty())
        .unwrap_or(parent);
    let list = super::pidl::from_path(cpu, &path, true)
        .map(|items| super::pidl::encode(&items))
        .unwrap_or_default();
    let data = LinkData {
        list,
        attributes: files::attributes(cpu, &path).unwrap_or(0),
        path,
        description: title.clone(),
        directory,
        arguments,
        icon: args.get(2).map(|i| unquote(i)).unwrap_or_default(),
        icon_index: args.get(3).and_then(|i| i.parse().ok()).unwrap_or(0),
        // SW_SHOWMINNOACTIVE for a minimized item, else SW_SHOWNORMAL.
        show: if args.get(8).is_some_and(|m| m != "0" && !m.is_empty()) {
            7
        } else {
            1
        },
        hotkey: args.get(7).and_then(|h| h.parse().ok()).unwrap_or(0),
        ..LinkData::default()
    };
    let target = format!("{}\\{}.lnk", dir, title);
    files::write_whole(cpu, &target, &link::write(&data)).is_ok()
}

/// Run the execute string `text`: the titles of the windows to open,
/// or None if a command failed or isn't Program Manager's.
pub fn execute(cpu: &mut Cpu, text: &str) -> Option<Vec<String>> {
    cpu.bus
        .log_string(&format!("[W32] Program Manager: {}", text));
    let commands = parse(text)?;
    let mut windows = Vec::new();
    for (name, args) in commands {
        let first = args.first().map(|a| unquote(a)).unwrap_or_default();
        match name.to_ascii_lowercase().as_str() {
            "creategroup" => {
                if first.is_empty() {
                    return None;
                }
                let (dir, _) = group_folder(cpu, &first);
                folders::make_directories(cpu, &dir);
                let (_, shown) = group_folder(cpu, &first);
                cpu.bus.win32.shell.group = Some(dir);
                windows.push(shown);
            }
            "showgroup" => {
                if args.len() < 2 {
                    return None;
                }
                let (dir, shown) = group_folder(cpu, &first);
                if files::attributes(cpu, &dir).is_err() {
                    return None;
                }
                cpu.bus.win32.shell.group = Some(dir);
                windows.push(shown);
            }
            "deletegroup" => {
                let (dir, _) = group_folder(cpu, &first);
                if first.is_empty() || !delete_tree(cpu, &dir) {
                    return None;
                }
            }
            "additem" => {
                let dir = cpu.bus.win32.shell.group.clone()?;
                if !add_item(cpu, &dir, &args) {
                    return None;
                }
            }
            "deleteitem" | "replaceitem" => {
                let dir = cpu.bus.win32.shell.group.clone()?;
                let path = format!("{}\\{}.lnk", dir, first);
                if files::delete_file(cpu, &path).is_err() {
                    return None;
                }
            }
            "reload" | "exitprogman" => {}
            _ => return None,
        }
    }
    Some(windows)
}

/// A request for the item `item`: the groups (item "Groups" or
/// "PROGMAN"), one name to a line, or the items of a group.
pub fn request(cpu: &mut Cpu, item: &str) -> Option<String> {
    let programs = folders::path(cpu, 0x02)?;
    let dir = if item.eq_ignore_ascii_case("Groups") || item.eq_ignore_ascii_case("PROGMAN") {
        programs
    } else {
        let (dir, _) = group_folder(cpu, item);
        files::attributes(cpu, &dir).ok()?;
        dir
    };
    let mut text = String::new();
    for entry in files::find(cpu, &dir, "*").unwrap_or_default() {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        let name = entry
            .name
            .strip_suffix(".lnk")
            .unwrap_or(&entry.name)
            .to_string();
        text.push_str(&name);
        text.push_str("\r\n");
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn commands_are_parsed_with_their_arguments() {
        let got = parse("[CreateGroup(Group3)][AddItem(\"C:\\A B\\x.exe\" -y,Name, ,1)] [Reload]");
        let want = vec![
            ("CreateGroup".to_string(), vec!["Group3".to_string()]),
            (
                "AddItem".to_string(),
                vec![
                    "\"C:\\A B\\x.exe\" -y".to_string(),
                    "Name".to_string(),
                    String::new(),
                    "1".to_string(),
                ],
            ),
            ("Reload".to_string(), vec![]),
        ];
        assert_eq!(got, Some(want));
    }

    #[test]
    fn a_broken_command_is_none() {
        assert_eq!(parse("[CreateGroup(x"), None);
        assert_eq!(parse("CreateGroup(x)"), None);
    }
}
