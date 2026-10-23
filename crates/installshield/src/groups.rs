//! A cabinet's file groups, read as Unshield reads them. The cabinet
//! descriptor holds the heads of 71 lists at 3Eh, each of 12-byte entries
//! (the offsets of a name, a group descriptor and the next entry); a group
//! descriptor starts with its name's offset and holds its first and last
//! file index at 4Ch and 50h in versions 0 and 5, at 16h and 1Ah in
//! version 6. Every offset is from the cabinet descriptor. A group names a
//! range of the file table, and that range is all the cabinet records of
//! which files a group installs.
//!
//! Groups never fail the open: what doesn't read is reported, in the
//! group's `problem`, the table's `problem` and each entry's `Membership`.

use crate::cabinet::HeaderReader;
use crate::{Error, Limits};
use std::collections::{HashMap, HashSet};

const LIST_OFFSET: u64 = 0x3e;
const LIST_COUNT: u64 = 71;
const LIST_ENTRY_SIZE: u64 = 12;

/// One file group a cabinet lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileGroup {
    /// Its place in `Cabinet::file_groups`.
    pub index: usize,
    /// Its name, as ISO 8859-1, or None when it doesn't read. A name is
    /// data: it isn't checked as a path.
    pub name: Option<String>,
    /// Its range of file indexes, both ends included, when it reads.
    pub first_file: Option<u32>,
    pub last_file: Option<u32>,
    /// Why its descriptor, name or range isn't usable.
    pub problem: Option<String>,
}

/// How an entry's groups read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MembershipKind {
    /// There are no file groups: a cabinet that lists none, or an
    /// InstallShield 3 archive.
    #[default]
    NoFileGroups,
    /// The groups all read, and no range holds the entry.
    None,
    /// The groups all read, and one range holds it.
    One,
    /// The groups all read, and more than one range holds it. The header
    /// doesn't say which of them installs it.
    Several,
    /// Not known: the group lists didn't read whole, or a group whose
    /// range is malformed may hold it.
    Undetermined,
}

/// The file groups whose ranges hold one entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Membership {
    pub kind: MembershipKind,
    /// Indexes into `Cabinet::file_groups` of the well-formed groups that
    /// hold it.
    pub groups: Vec<usize>,
    /// Indexes of malformed groups that may hold it.
    pub malformed: Vec<usize>,
}

pub(crate) struct GroupTable {
    pub(crate) groups: Vec<FileGroup>,
    /// Why the lists didn't read whole.
    pub(crate) problem: Option<String>,
    /// Whether the descriptor lists any group, or its lists didn't read.
    listed: bool,
    file_count: usize,
    max_pairs: usize,
}

impl GroupTable {
    pub(crate) fn read(
        reader: &mut HeaderReader,
        descriptor: u64,
        major: u32,
        file_count: usize,
        limits: &Limits,
    ) -> Result<GroupTable, Error> {
        let mut groups = Vec::new();
        // The entries every list has reached, and those this list has. A
        // list that comes back to its own entry loops; one that reaches an
        // entry an earlier list read joins that list, so it ends there and
        // each group is listed once.
        let mut read = HashSet::new();
        let mut visited = HashSet::new();
        let mut names: HashMap<u32, (Option<String>, Option<String>)> = HashMap::new();
        let mut listed = false;
        let (first_at, last_at) = if major == 6 {
            (0x16, 0x1a)
        } else {
            (0x4c, 0x50)
        };
        let table = |groups, problem: String| GroupTable {
            groups,
            problem: Some(problem),
            listed: true,
            file_count,
            max_pairs: limits.max_files,
        };
        for list in 0..LIST_COUNT {
            visited.clear();
            let mut next = match reader.u32(descriptor + LIST_OFFSET + 4 * list) {
                Ok(v) => v,
                Err(Error::Invalid(e)) => {
                    return Ok(table(
                        groups,
                        format!(
                            "The cabinet descriptor's file group lists don't read. {}",
                            e
                        ),
                    ));
                }
                Err(e) => return Err(e),
            };
            while next != 0 {
                listed = true;
                if !visited.insert(next) {
                    return Ok(table(
                        groups,
                        format!(
                            "File group list {} comes back to its entry at descriptor offset {:#x}.",
                            list, next
                        ),
                    ));
                }
                if !read.insert(next) {
                    break;
                }
                if groups.len() == limits.max_files {
                    return Ok(table(
                        groups,
                        format!(
                            "The cabinet lists more than {} file groups, the file limit; the rest aren't read.",
                            limits.max_files
                        ),
                    ));
                }
                let entry = descriptor + next as u64;
                let fields = reader
                    .require(entry, LIST_ENTRY_SIZE, "file group list entry")
                    .and_then(|()| Ok((reader.u32(entry + 4)?, reader.u32(entry + 8)?)));
                let group_descriptor = match fields {
                    Ok((group, following)) => {
                        next = following;
                        group
                    }
                    Err(Error::Invalid(e)) => {
                        return Ok(table(
                            groups,
                            format!("An entry of file group list {} doesn't read. {}", list, e),
                        ));
                    }
                    Err(e) => return Err(e),
                };
                let index = groups.len();
                let group = read_group(
                    reader,
                    descriptor,
                    index,
                    group_descriptor,
                    (first_at, last_at),
                    file_count,
                    &mut names,
                )?;
                groups.push(group);
            }
        }
        Ok(GroupTable {
            groups,
            problem: None,
            listed,
            file_count,
            max_pairs: limits.max_files,
        })
    }

    /// The membership of each of `entries` (file-table indexes, ascending
    /// and distinct), and the problem with the groups, the pair limit
    /// included: the ranges together may name at most the file limit of
    /// (entry, group) pairs, which bounds what this builds however they
    /// overlap.
    pub(crate) fn memberships(
        &self,
        entries: &[usize],
    ) -> (HashMap<usize, Membership>, Option<String>) {
        let mut result = HashMap::with_capacity(entries.len());
        if !self.listed && self.problem.is_none() {
            for &entry in entries {
                result.insert(entry, Membership::default());
            }
            return (result, None);
        }
        let mut held: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut suspected: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut pairs = 0usize;
        let mut exhausted = false;
        for group in &self.groups {
            let (from, to, malformed) = match (group.first_file, group.last_file) {
                (Some(first), Some(last)) if first <= last => {
                    // A range inside the table holds its entries; one that
                    // reaches past it may hold those from its first on.
                    if (last as usize) < self.file_count {
                        (first as usize, last as usize, false)
                    } else {
                        (first as usize, usize::MAX, true)
                    }
                }
                _ => (0, usize::MAX, true),
            };
            if from >= self.file_count {
                continue;
            }
            let start = entries.partition_point(|&e| e < from);
            for &entry in entries[start..].iter().take_while(|&&e| e <= to) {
                pairs += 1;
                if pairs > self.max_pairs {
                    exhausted = true;
                    break;
                }
                let target = if malformed { &mut suspected } else { &mut held };
                target.entry(entry).or_default().push(group.index);
            }
            if exhausted {
                break;
            }
        }
        let mut problem = self.problem.clone();
        if exhausted {
            let too_many = format!(
                "The file groups' ranges name more than {} entries with their groups, the file limit; memberships aren't read.",
                self.max_pairs
            );
            problem = Some(match problem {
                Some(p) => format!("{} {}", p, too_many),
                None => too_many,
            });
            held.clear();
            suspected.clear();
        }
        for &entry in entries {
            let groups = held.remove(&entry).unwrap_or_default();
            let malformed = suspected.remove(&entry).unwrap_or_default();
            let kind = if problem.is_some() || !malformed.is_empty() {
                MembershipKind::Undetermined
            } else {
                match groups.len() {
                    0 => MembershipKind::None,
                    1 => MembershipKind::One,
                    _ => MembershipKind::Several,
                }
            };
            result.insert(
                entry,
                Membership {
                    kind,
                    groups,
                    malformed,
                },
            );
        }
        (result, problem)
    }
}

fn read_group(
    reader: &mut HeaderReader,
    descriptor: u64,
    index: usize,
    offset: u32,
    (first_at, last_at): (u64, u64),
    file_count: usize,
    names: &mut HashMap<u32, (Option<String>, Option<String>)>,
) -> Result<FileGroup, Error> {
    let bad = |problem: String| FileGroup {
        index,
        name: None,
        first_file: None,
        last_file: None,
        problem: Some(problem),
    };
    if offset == 0 {
        return Ok(bad(
            "The group's list entry names no group descriptor.".to_string()
        ));
    }
    let at = descriptor + offset as u64;
    let fields = reader
        .require(at, last_at + 4, "file group descriptor")
        .and_then(|()| {
            Ok((
                reader.u32(at)?,
                reader.u32(at + first_at)?,
                reader.u32(at + last_at)?,
            ))
        });
    let (name_offset, first, last) = match fields {
        Ok(f) => f,
        Err(Error::Invalid(e)) => {
            return Ok(bad(format!("The group's descriptor doesn't read. {}", e)));
        }
        Err(e) => return Err(e),
    };
    let mut problems = Vec::new();
    let mut name = None;
    if name_offset == 0 {
        problems.push("The group has no name.".to_string());
    } else {
        let (read, problem) = match names.entry(name_offset) {
            std::collections::hash_map::Entry::Occupied(known) => known.get().clone(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                let read = match reader.string(descriptor + name_offset as u64, "file group name") {
                    Ok(n) => (Some(n), None),
                    Err(Error::Invalid(e)) => {
                        (None, Some(format!("The group's name doesn't read. {}", e)))
                    }
                    Err(e) => return Err(e),
                };
                slot.insert(read).clone()
            }
        };
        name = read;
        problems.extend(problem);
    }
    if first > last {
        problems.push(format!(
            "Its range, files {} to {}, is reversed.",
            first, last
        ));
    } else if last as usize >= file_count {
        problems.push(format!(
            "Its range, files {} to {}, reaches past the {}-entry file table.",
            first, last, file_count
        ));
    }
    Ok(FileGroup {
        index,
        name,
        first_file: Some(first),
        last_file: Some(last),
        problem: (!problems.is_empty()).then(|| problems.join(" ")),
    })
}
