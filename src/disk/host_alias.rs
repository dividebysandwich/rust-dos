//! The 8.3 aliases of the entries of host folders, kept while the emulator
//! runs.
//!
//! Windows 9x's VFAT keeps an entry's alias in its directory entry: an
//! entry keeps its alias until it is renamed, a renamed entry gets the
//! first alias that is free, its old one held until the rename is done,
//! and a new entry never takes an alias another entry has. Checked on
//! Windows 98 SE in 86Box: "Parent two" (PARENT~1) renamed to "Parent
//! moved" is PARENT~2, and "Parent a" made beside "Parent two" is PARENT~2
//! too, while "Parent two" keeps PARENT~1. A rename that changes only the
//! case keeps the alias: "Read Me.txt" (README~1.TXT) renamed to "READ
//! ME.TXT" is README~1.TXT still.
//!
//! A host folder keeps no aliases, so `short_names` makes them from the
//! names a folder has, sorted. Made afresh on each read, they would follow
//! the names: a renamed folder would get its old alias again, and a new
//! one that sorts first would take another's. So the aliases a folder's
//! entries got are kept here, by host path, and each read of the folder
//! gives the names it has now the aliases they had (`HostAliases::assign`):
//! - a name that was there keeps its alias;
//! - a new name that is a name gone in another case (a rename of the
//!   case) takes the alias of the name gone;
//! - another new name gets the first alias free, not one a name gone had:
//!   a rename's new name is read with its old one gone, so the old alias
//!   is held as VFAT holds it during the rename. A name that is 8.3
//!   already is its own alias unless a name there has it, so a rename to
//!   the entry's own alias keeps it;
//! - a name gone loses its alias.
//!
//! The guest's deletes are read before its next create in the folder (the
//! create looks the name up first), so a deleted entry's alias is free for
//! it, as on Windows 9x. A renamed or removed folder takes its kept
//! aliases along or drops them (`moved`, `removed`).
//!
//! A folder whose entries all have their own names in upper case as their
//! aliases, as most DOS programs' do, keeps nothing: `short_names` makes
//! the same aliases for it afresh.

use super::*;
use std::collections::HashSet;

/// The aliases of one host folder's entries.
struct Folder {
    /// Each entry's alias, by its host name.
    aliases: HashMap<String, String>,
    /// The aliases differ from those `short_names` makes for the names
    /// afresh, so a save state has to keep them (`saved`).
    moved: bool,
}

/// A host folder's aliases as a save state keeps them: the drive, the
/// folder's path from the drive's root in host names, and its entries'
/// (host name, alias).
pub(crate) type SavedFolder = (u8, Vec<String>, Vec<(String, String)>);

#[derive(Default)]
pub(super) struct HostAliases {
    folders: RefCell<HashMap<PathBuf, Folder>>,
}

impl HostAliases {
    /// The aliases of `names`, the entries the host folder `dir` has now,
    /// in the order `read_host_entries` sorts them, as the folder's
    /// entries keep them (see the module's comment).
    pub(super) fn assign(&self, dir: &Path, names: &[String]) -> Vec<String> {
        let mut folders = self.folders.borrow_mut();
        let Some(folder) = folders.get_mut(dir) else {
            let aliases = short_names(names);
            if !Self::all_own(names, &aliases) {
                let folder = Folder {
                    aliases: names.iter().cloned().zip(aliases.iter().cloned()).collect(),
                    moved: false,
                };
                folders.insert(dir.to_path_buf(), folder);
            }
            return aliases;
        };
        let unchanged = names.len() == folder.aliases.len()
            && names.iter().all(|name| folder.aliases.contains_key(name));
        if unchanged {
            return names
                .iter()
                .map(|name| folder.aliases[name].clone())
                .collect();
        }
        let aliases = Self::reconcile(&folder.aliases, names);
        if Self::all_own(names, &aliases) {
            folders.remove(dir);
        } else {
            folder.moved = aliases != short_names(names);
            folder.aliases = names.iter().cloned().zip(aliases.iter().cloned()).collect();
        }
        aliases
    }

    /// Whether each of `names` has its own name in upper case as its
    /// alias.
    fn all_own(names: &[String], aliases: &[String]) -> bool {
        names
            .iter()
            .zip(aliases)
            .all(|(name, alias)| name.to_ascii_uppercase() == *alias)
    }

    /// The aliases of `names` in a folder whose entries had `kept`.
    fn reconcile(kept: &HashMap<String, String>, names: &[String]) -> Vec<String> {
        let present: HashSet<&str> = names.iter().map(String::as_str).collect();
        let mut gone: Vec<(String, &String)> = kept
            .iter()
            .filter(|(name, _)| !present.contains(name.as_str()))
            .map(|(name, alias)| (name.to_lowercase(), alias))
            .collect();
        // In the order of the names gone, so that two new names that are
        // both one gone in another case (a case-sensitive host) take the
        // same one whatever order the map has.
        gone.sort();
        let mut live: HashSet<String> = HashSet::new();
        let mut aliases: Vec<Option<String>> = names
            .iter()
            .map(|name| {
                let alias = kept.get(name)?;
                live.insert(alias.clone());
                Some(alias.clone())
            })
            .collect();
        for (name, alias) in names.iter().zip(aliases.iter_mut()) {
            if alias.is_some() {
                continue;
            }
            let lower = name.to_lowercase();
            if let Some(at) = gone
                .iter()
                .position(|(was, a)| *was == lower && !live.contains(*a))
            {
                let (_, recased) = gone.remove(at);
                live.insert(recased.clone());
                *alias = Some(recased.clone());
            }
        }
        let held: HashSet<String> = gone.iter().map(|(_, alias)| (*alias).clone()).collect();
        let new: Vec<&String> = names
            .iter()
            .zip(&aliases)
            .filter(|(_, alias)| alias.is_none())
            .map(|(name, _)| name)
            .collect();
        let mut made = short_names_beside(&new, &live, &held).into_iter();
        aliases
            .into_iter()
            .map(|alias| alias.or_else(|| made.next()).unwrap_or_default())
            .collect()
    }

    /// The guest renamed the host file or folder `from` to `to`: the
    /// folders at and under `from` keep their entries' aliases at `to`.
    pub(super) fn moved(&self, from: &Path, to: &Path) {
        let mut folders = self.folders.borrow_mut();
        let under: Vec<PathBuf> = folders
            .keys()
            .filter(|dir| dir.starts_with(from))
            .cloned()
            .collect();
        for dir in under {
            let Ok(below) = dir.strip_prefix(from) else {
                continue;
            };
            let target = if below.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(below)
            };
            if let Some(folder) = folders.remove(&dir) {
                folders.insert(target, folder);
            }
        }
    }

    /// The guest removed the host folder `dir`: a folder made there later
    /// starts without its aliases.
    pub(super) fn removed(&self, dir: &Path) {
        self.folders
            .borrow_mut()
            .retain(|kept, _| !kept.starts_with(dir));
    }

    /// The kept aliases a save state needs, for the host drives `roots`
    /// (drive, root): those of the folders under a root whose aliases
    /// differ from what `short_names` makes for them afresh, sorted.
    pub(super) fn saved(&self, roots: &[(u8, PathBuf)]) -> Vec<SavedFolder> {
        let folders = self.folders.borrow();
        let mut saved = Vec::new();
        for (dir, folder) in folders.iter().filter(|(_, f)| f.moved) {
            let Some((drive, below)) = roots
                .iter()
                .find_map(|(drive, root)| Some((*drive, dir.strip_prefix(root).ok()?)))
            else {
                continue;
            };
            let parts = below
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let mut aliases: Vec<(String, String)> = folder
                .aliases
                .iter()
                .map(|(n, a)| (n.clone(), a.clone()))
                .collect();
            aliases.sort();
            saved.push((drive, parts, aliases));
        }
        saved.sort();
        saved
    }

    /// Put the kept aliases of a state loaded in place of those kept: its
    /// folders under the host drives `roots` (drive, root) as it has them.
    /// The other folders' aliases are made afresh, as the run that saved
    /// it had them.
    pub(super) fn load(&self, saved: &[SavedFolder], roots: &[(u8, PathBuf)]) {
        let mut folders = self.folders.borrow_mut();
        folders.clear();
        for (drive, parts, aliases) in saved {
            let Some((_, root)) = roots.iter().find(|(d, _)| d == drive) else {
                continue;
            };
            let dir = parts.iter().fold(root.clone(), |dir, part| dir.join(part));
            let folder = Folder {
                aliases: aliases.iter().cloned().collect(),
                moved: true,
            };
            folders.insert(dir, folder);
        }
    }
}
