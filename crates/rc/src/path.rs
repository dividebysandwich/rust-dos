//! DOS paths as RC keeps them: a drive, a colon and a backslash, then the
//! directories, upper case, with no backslash at the end ("C:\GAMES"),
//! except at a root ("C:\").

/// The drive of a path, 0 for A:.
pub fn drive(path: &str) -> u8 {
    path.bytes()
        .next()
        .map_or(2, |b| b.to_ascii_uppercase().wrapping_sub(b'A'))
}

/// The root directory of a drive.
pub fn root(drive: u8) -> String {
    format!("{}:\\", (b'A' + drive) as char)
}

pub fn is_root(path: &str) -> bool {
    path.len() <= 3
}

/// `name` in directory `dir`.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('\\') {
        format!("{}{}", dir, name)
    } else {
        format!("{}\\{}", dir, name)
    }
}

/// The directory a path is in, None at a root.
pub fn parent(path: &str) -> Option<String> {
    if is_root(path) {
        return None;
    }
    let i = path.rfind('\\')?;
    Some(if i <= 2 {
        path[..3].to_string()
    } else {
        path[..i].to_string()
    })
}

/// A path as NC shows a file's: the directory in upper case, the name in
/// lower case.
pub fn shown(path: &str) -> String {
    match parent(path) {
        Some(dir) => join(&dir.to_ascii_uppercase(), &name(path).to_ascii_lowercase()),
        None => path.to_ascii_uppercase(),
    }
}

/// The last part of a path.
pub fn name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

/// What was typed for a path, from directory `dir`: with or without a
/// drive, from the root or not, "." and ".." taken out. A drive alone
/// ("D:") means that drive's `current` directory.
pub fn qualify(dir: &str, typed: &str, current: impl Fn(u8) -> String) -> String {
    let typed = typed.trim().replace('/', "\\");
    let bytes = typed.as_bytes();
    let (base, rest) = if bytes.len() >= 2 && bytes[1] == b':' {
        let d = bytes[0].to_ascii_uppercase().wrapping_sub(b'A');
        let rest = &typed[2..];
        if rest.starts_with('\\') {
            (root(d), rest.to_string())
        } else {
            (current(d), rest.to_string())
        }
    } else if typed.starts_with('\\') {
        (root(drive(dir)), typed.clone())
    } else {
        (dir.to_string(), typed.clone())
    };
    let mut parts: Vec<String> = base[3..]
        .split('\\')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p.to_ascii_uppercase()),
        }
    }
    format!("{}{}", &base[..3], parts.join("\\")).to_ascii_uppercase()
}

/// Whether `path` is `dir` or inside it.
pub fn within(path: &str, dir: &str) -> bool {
    let (p, d) = (path.to_ascii_uppercase(), dir.to_ascii_uppercase());
    p == d || p.starts_with(&format!("{}\\", d.trim_end_matches('\\')))
}

/// A name's base and extension: "README.TXT" is ("README", "TXT").
pub fn split_name(name: &str) -> (&str, &str) {
    if name == ".." {
        return (name, "");
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// Whether a name matches a DOS wildcard pattern, as DIR matches them:
/// the base and the extension each on their own, `*` for the rest of a
/// part, `?` for one character, "*.*" for every name.
pub fn matches(pattern: &str, name: &str) -> bool {
    let (pb, pe) = match pattern.rfind('.') {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => (pattern, ""),
    };
    let (nb, ne) = split_name(name);
    part_matches(pb, nb) && (part_matches(pe, ne) || (pe.is_empty() && ne.is_empty()))
}

fn part_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_uppercase().chars().collect();
    let n: Vec<char> = name.to_ascii_uppercase().chars().collect();
    let mut i = 0;
    for &c in &p {
        match c {
            '*' => return true,
            '?' => i += 1,
            c => {
                if n.get(i) != Some(&c) {
                    return false;
                }
                i += 1;
            }
        }
    }
    i >= n.len()
}

/// A name for `name` from a template with wildcards, as COPY and REN make
/// them: "*.BAK" gives README.BAK.
pub fn template(pattern: &str, name: &str) -> String {
    let (pb, pe) = match pattern.rfind('.') {
        Some(i) => (&pattern[..i], Some(&pattern[i + 1..])),
        None => (pattern, None),
    };
    let (nb, ne) = split_name(name);
    let base = fill(pb, nb);
    match pe {
        Some(pe) => {
            let ext = fill(pe, ne);
            if ext.is_empty() {
                base
            } else {
                format!("{}.{}", base, ext)
            }
        }
        None => base,
    }
}

fn fill(pattern: &str, name: &str) -> String {
    let n: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, c) in pattern.chars().enumerate() {
        match c {
            '*' => {
                out.extend(n.iter().skip(i));
                break;
            }
            '?' => {
                if let Some(&c) = n.get(i) {
                    out.push(c);
                }
            }
            c => out.push(c),
        }
    }
    out.to_ascii_uppercase()
}

pub fn has_wildcards(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let cur = |d: u8| format!("{}:\\WORK", (b'A' + d) as char);
        assert_eq!(qualify("C:\\GAMES", "SUB", cur), "C:\\GAMES\\SUB");
        assert_eq!(qualify("C:\\GAMES", "..", cur), "C:\\");
        assert_eq!(qualify("C:\\GAMES", "\\X\\y", cur), "C:\\X\\Y");
        assert_eq!(qualify("C:\\GAMES", "d:", cur), "D:\\WORK");
        assert_eq!(qualify("C:\\GAMES", "D:\\", cur), "D:\\");
        assert_eq!(parent("C:\\GAMES\\SUB").as_deref(), Some("C:\\GAMES"));
        assert_eq!(parent("C:\\GAMES").as_deref(), Some("C:\\"));
        assert_eq!(parent("C:\\"), None);
        assert!(within("C:\\A\\B", "C:\\A"));
        assert!(!within("C:\\AB", "C:\\A"));
        assert!(within("C:\\A", "C:\\"));
    }

    #[test]
    fn wildcards() {
        assert!(matches("*.*", "README.TXT"));
        assert!(matches("*.*", "GAMES"));
        assert!(matches("*.txt", "readme.TXT"));
        assert!(!matches("*.txt", "readme.doc"));
        assert!(matches("R?ADME.*", "README.TXT"));
        assert!(matches("*", "GAMES"));
        assert!(!matches("*", "README.TXT"));
        assert_eq!(template("*.BAK", "README.TXT"), "README.BAK");
        assert_eq!(template("A*.*", "README.TXT"), "AEADME.TXT");
    }
}
