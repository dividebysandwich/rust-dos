//! File mappings' memory (CreateFileMapping, MapViewOfFile): as on
//! Windows 95, a mapping has one place in the shared arena, the same in
//! every process, and its views are its pages mapped there in each
//! process that maps it. Its pages are the system's, not a process's:
//! they go back to the pool when the mapping's object is gone and no view
//! of it is left.

/// Where mappings are put in the shared arena: above what the processes
/// take there for their threads' TIBs, which they take from its bottom,
/// and below the built-in DLLs.
pub const ARENA: (u32, u32) = (0xA000_0000, 0xBF00_0000);

#[derive(Clone, Debug, Default)]
pub struct Section {
    /// Its kernel object.
    pub object: u32,
    /// Its address in the shared arena and its size.
    pub base: u32,
    pub size: u32,
    /// The protection it was made with (a PAGE_ value), and whether its
    /// pages are only reserved until VirtualAlloc commits them
    /// (SEC_RESERVE).
    pub protect: u32,
    pub reserve: bool,
    /// Each page's physical page, or 0 where none is committed.
    pub pages: Vec<u32>,
    /// The file it maps, as the object of the program's handle of it,
    /// which the mapping holds, and its entry in DOS's file table; or
    /// None for a mapping of the swap file.
    pub file: Option<(u32, u16)>,
    /// Its views: the process's id and the view's address.
    pub views: Vec<(u32, u32)>,
}

impl Section {
    /// The end of its place: its pages' end.
    pub fn end(&self) -> u32 {
        self.base + self.pages.len() as u32 * super::memory::PAGE
    }
}

impl super::Win32 {
    /// Whether the physical page `phys` is a file mapping's, which a
    /// process's memory gives back to the pool only with the mapping.
    pub fn shared(&self, phys: u32) -> bool {
        self.sections
            .iter()
            .flatten()
            .any(|s| s.pages.contains(&phys))
    }
}

crate::state_fields!(Section {
    object,
    base,
    size,
    protect,
    reserve,
    pages,
    file,
    views
});
