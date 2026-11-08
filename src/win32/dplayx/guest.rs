//! DirectPlay's structures in the program's memory: names, session
//! descriptions and the system messages, read and written as the
//! interface the program called has them, ANSI (IDirectPlay, 2A, 3A,
//! 4A) or Unicode (2, 3, 4).
//!
//! A structure DirectPlay hands out (a message, a description, a name)
//! is one block: the structure, then the strings and data its pointers
//! point at (`Blob`).

use super::session::{Entity, Info, Sys};
use crate::cpu::Cpu;
use crate::win32::kernel32::{ansi_to_wide, wide_to_ansi};

/// The longest string read.
const LONGEST: u32 = 0x1_0000;

pub const DPNAME_SIZE: u32 = 16;
pub const DESC2_SIZE: u32 = 80;
pub const DESC1_SIZE: u32 = 124;
/// IDirectPlay's names and session description, in fixed arrays.
const SHORT_NAME: usize = 20;
const LONG_NAME: usize = 52;
const SESSION_NAME: usize = 32;
const PASSWORD: usize = 16;

/// System messages' types.
pub const DPSYS_CREATEPLAYERORGROUP: u32 = 0x0003;
pub const DPSYS_DESTROYPLAYERORGROUP: u32 = 0x0005;
pub const DPSYS_ADDPLAYERTOGROUP: u32 = 0x0007;
pub const DPSYS_DELETEPLAYERFROMGROUP: u32 = 0x0021;
pub const DPSYS_SESSIONLOST: u32 = 0x0031;
pub const DPSYS_HOST: u32 = 0x0101;
pub const DPSYS_SETPLAYERORGROUPDATA: u32 = 0x0102;
pub const DPSYS_SETPLAYERORGROUPNAME: u32 = 0x0103;
pub const DPSYS_SETSESSIONDESC: u32 = 0x0104;
pub const DPSYS_ADDGROUPTOGROUP: u32 = 0x0105;
pub const DPSYS_DELETEGROUPFROMGROUP: u32 = 0x0106;
pub const DPSYS_CHAT: u32 = 0x0109;
pub const DPSYS_SETGROUPOWNER: u32 = 0x010A;
pub const DPSYS_SENDCOMPLETE: u32 = 0x010D;
/// IDirectPlay's own: a group went.
const DPSYS_DELETEGROUP: u32 = 0x0020;

const DPPLAYERTYPE_GROUP: u32 = 0;
const DPPLAYERTYPE_PLAYER: u32 = 1;

/// The string at `at` (Some(None) for NULL), or None if a page of it
/// isn't there.
pub fn read_str(cpu: &mut Cpu, at: u32, wide: bool) -> Option<Option<Vec<u16>>> {
    if at == 0 {
        return Some(None);
    }
    let text = if wide {
        let mut units = Vec::new();
        let mut p = at;
        loop {
            if !cpu.bus.guest_probe(p, 2, false) {
                return None;
            }
            let c = cpu.bus.guest_read_16(p);
            if c == 0 || units.len() as u32 >= LONGEST {
                break;
            }
            units.push(c);
            p = p.wrapping_add(2);
        }
        units
    } else {
        let bytes = cpu.bus.guest_read_asciiz(at, LONGEST as usize);
        if cpu.bus.guest_faulted() {
            return None;
        }
        ansi_to_wide(&bytes)
    };
    Some(Some(text))
}

/// A string as the interface has it, with its zero.
pub fn str_bytes(text: &[u16], wide: bool) -> Vec<u8> {
    if wide {
        let mut out: Vec<u8> = text.iter().flat_map(|c| c.to_le_bytes()).collect();
        out.extend_from_slice(&[0, 0]);
        out
    } else {
        let mut out = wide_to_ansi(text);
        out.push(0);
        out
    }
}

/// A player's or group's short and long names, either of which may be
/// missing.
pub type Names = (Option<Vec<u16>>, Option<Vec<u16>>);

/// The names of the DPNAME at `at`: none for NULL.
pub fn read_name(cpu: &mut Cpu, at: u32, wide: bool) -> Option<Names> {
    if at == 0 {
        return Some((None, None));
    }
    if !cpu.bus.guest_probe(at, DPNAME_SIZE as usize, false) {
        return None;
    }
    let (short, long) = (
        cpu.bus.guest_read_32(at + 8),
        cpu.bus.guest_read_32(at + 12),
    );
    Some((read_str(cpu, short, wide)?, read_str(cpu, long, wide)?))
}

/// The bytes at `at`, `size` of them.
pub fn read_data(cpu: &mut Cpu, at: u32, size: u32) -> Option<Vec<u8>> {
    if size == 0 {
        return Some(Vec::new());
    }
    if at == 0 || size > super::MAX_DATA || !cpu.bus.guest_probe(at, size as usize, false) {
        return None;
    }
    let mut data = vec![0; size as usize];
    cpu.bus.guest_read_bytes(at, &mut data);
    Some(data)
}

/// A structure and what its pointers point at, written as one block.
#[derive(Default)]
pub struct Blob {
    pub bytes: Vec<u8>,
    /// The pointers: where each is, and where in the block it points.
    pointers: Vec<(usize, usize)>,
}

impl Blob {
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(v);
        self
    }

    /// A pointer, filled in by `attach`: NULL until then.
    pub fn pointer(&mut self) -> usize {
        self.bytes.extend_from_slice(&[0; 4]);
        self.bytes.len() - 4
    }

    /// Put `data` at the end and point `pointer` at it.
    pub fn attach(&mut self, pointer: usize, data: &[u8]) {
        self.pointers.push((pointer, self.bytes.len()));
        self.bytes.extend_from_slice(data);
    }

    /// Point `pointer` at the string, unless there is none.
    pub fn attach_str(&mut self, pointer: usize, text: &Option<Vec<u16>>, wide: bool) {
        if let Some(t) = text {
            self.attach(pointer, &str_bytes(t, wide));
        }
    }

    /// Point `pointer` at the data, unless there is none.
    pub fn attach_data(&mut self, pointer: usize, data: &[u8]) {
        if !data.is_empty() {
            self.attach(pointer, data);
        }
    }

    pub fn len(&self) -> u32 {
        self.bytes.len() as u32
    }

    /// The block as it is at `at`.
    pub fn placed(&self, at: u32) -> Vec<u8> {
        let mut bytes = self.bytes.clone();
        for &(pointer, target) in &self.pointers {
            let value = at.wrapping_add(target as u32);
            bytes[pointer..pointer + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    /// Write it at `at`: false if a page isn't there.
    pub fn write(&self, cpu: &mut Cpu, at: u32) -> bool {
        if !cpu.bus.guest_probe(at, self.bytes.len(), true) {
            return false;
        }
        cpu.bus.guest_write_bytes(at, &self.placed(at));
        true
    }
}

/// A DPNAME of `short` and `long`, its strings at the end of `blob`
/// once `finish` attaches them.
pub struct NameAt {
    short: usize,
    long: usize,
}

pub fn name(blob: &mut Blob) -> NameAt {
    blob.u32(DPNAME_SIZE).u32(0);
    let short = blob.pointer();
    let long = blob.pointer();
    NameAt { short, long }
}

impl NameAt {
    pub fn finish(
        self,
        blob: &mut Blob,
        short: &Option<Vec<u16>>,
        long: &Option<Vec<u16>>,
        wide: bool,
    ) {
        blob.attach_str(self.short, short, wide);
        blob.attach_str(self.long, long, wide);
    }
}

/// A DPNAME alone, with its strings.
pub fn name_blob(short: &Option<Vec<u16>>, long: &Option<Vec<u16>>, wide: bool) -> Blob {
    let mut blob = Blob::default();
    let at = name(&mut blob);
    at.finish(&mut blob, short, long, wide);
    blob
}

/// A DPSESSIONDESC2 read from the program: the description, its
/// password, and None if dwSize isn't the structure's.
pub fn read_desc2(cpu: &mut Cpu, at: u32, wide: bool) -> Option<(Info, Vec<u16>)> {
    if at == 0 || !cpu.bus.guest_probe(at, DESC2_SIZE as usize, false) {
        return None;
    }
    if cpu.bus.guest_read_32(at) != DESC2_SIZE {
        return None;
    }
    let mut instance = [0; 16];
    let mut application = [0; 16];
    cpu.bus.guest_read_bytes(at + 8, &mut instance);
    cpu.bus.guest_read_bytes(at + 24, &mut application);
    let (name_at, password_at) = (
        cpu.bus.guest_read_32(at + 48),
        cpu.bus.guest_read_32(at + 52),
    );
    let name = read_str(cpu, name_at, wide)?;
    let password = read_str(cpu, password_at, wide)?.unwrap_or_default();
    let info = Info {
        flags: cpu.bus.guest_read_32(at + 4),
        instance,
        application,
        max_players: cpu.bus.guest_read_32(at + 40),
        current_players: cpu.bus.guest_read_32(at + 44),
        name,
        has_password: !password.is_empty(),
        user: [
            cpu.bus.guest_read_32(at + 64),
            cpu.bus.guest_read_32(at + 68),
            cpu.bus.guest_read_32(at + 72),
            cpu.bus.guest_read_32(at + 76),
        ],
    };
    Some((info, password))
}

/// A DPSESSIONDESC2 to hand out: the password goes only to members.
pub fn desc2(blob: &mut Blob, info: &Info, password: &[u16], wide: bool) {
    blob.u32(DESC2_SIZE)
        .u32(info.flags)
        .raw(&info.instance)
        .raw(&info.application)
        .u32(info.max_players)
        .u32(info.current_players);
    let name = blob.pointer();
    let pass = blob.pointer();
    blob.u32(0).u32(0);
    for u in info.user {
        blob.u32(u);
    }
    blob.attach_str(name, &info.name, wide);
    let password = (!password.is_empty()).then(|| password.to_vec());
    blob.attach_str(pass, &password, wide);
}

/// IDirectPlay's DPSESSIONDESC read from the program: the description
/// (its guidSession is the application's GUID, its dwSession the
/// session's number), the password and the flags (DPOPEN_*).
pub fn read_desc1(cpu: &mut Cpu, at: u32) -> Option<(Info, u32, Vec<u16>)> {
    if at == 0 || !cpu.bus.guest_probe(at, DESC1_SIZE as usize, false) {
        return None;
    }
    let mut application = [0; 16];
    cpu.bus.guest_read_bytes(at + 4, &mut application);
    let session = cpu.bus.guest_read_32(at + 20);
    let fixed = |cpu: &mut Cpu, at: u32, len: usize| {
        let mut bytes = vec![0; len];
        cpu.bus.guest_read_bytes(at, &mut bytes);
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
        ansi_to_wide(&bytes[..end])
    };
    let name = fixed(cpu, at + 36, SESSION_NAME);
    let password = fixed(cpu, at + 88, PASSWORD);
    let info = Info {
        flags: 0,
        instance: session_guid(session),
        application,
        max_players: cpu.bus.guest_read_32(at + 24),
        current_players: 0,
        name: Some(name),
        has_password: !password.is_empty(),
        user: [
            cpu.bus.guest_read_32(at + 108),
            cpu.bus.guest_read_32(at + 112),
            cpu.bus.guest_read_32(at + 116),
            cpu.bus.guest_read_32(at + 120),
        ],
    };
    Some((info, cpu.bus.guest_read_32(at + 32), password))
}

/// IDirectPlay's number for a session: the first four bytes of its
/// instance GUID.
pub fn session_number(instance: &[u8; 16]) -> u32 {
    u32::from_le_bytes(instance[..4].try_into().unwrap())
}

/// An instance GUID that only says the session's number, for matching
/// against those found.
fn session_guid(number: u32) -> [u8; 16] {
    let mut guid = [0; 16];
    guid[..4].copy_from_slice(&number.to_le_bytes());
    guid
}

/// IDirectPlay's DPSESSIONDESC of a session.
pub fn desc1(info: &Info) -> Vec<u8> {
    let mut out = Vec::with_capacity(DESC1_SIZE as usize);
    let fixed = |out: &mut Vec<u8>, text: &[u16], len: usize| {
        let mut bytes = wide_to_ansi(text);
        bytes.truncate(len - 1);
        bytes.resize(len, 0);
        out.extend_from_slice(&bytes);
    };
    out.extend_from_slice(&DESC1_SIZE.to_le_bytes());
    out.extend_from_slice(&info.application);
    out.extend_from_slice(&session_number(&info.instance).to_le_bytes());
    out.extend_from_slice(&info.max_players.to_le_bytes());
    out.extend_from_slice(&info.current_players.to_le_bytes());
    // dwFlags is for Open's DPOPEN_* flags.
    out.extend_from_slice(&0u32.to_le_bytes());
    fixed(&mut out, info.name.as_deref().unwrap_or(&[]), SESSION_NAME);
    out.extend_from_slice(&[0; 16 + 4]);
    out.extend_from_slice(&[0; PASSWORD]);
    out.extend_from_slice(&[0; 4]);
    for u in info.user {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

fn player_type(e: &Entity) -> u32 {
    if e.group {
        DPPLAYERTYPE_GROUP
    } else {
        DPPLAYERTYPE_PLAYER
    }
}

/// A system message as IDirectPlay2 and later write it.
pub fn system2(sys: &Sys, wide: bool) -> Blob {
    let mut b = Blob::default();
    match sys {
        Sys::Create { entity, current } => {
            b.u32(DPSYS_CREATEPLAYERORGROUP)
                .u32(player_type(entity))
                .u32(entity.id)
                .u32(*current);
            let data = b.pointer();
            b.u32(entity.data.len() as u32);
            let n = name(&mut b);
            b.u32(entity.parent).u32(entity.reported_flags());
            n.finish(&mut b, &entity.short, &entity.long, wide);
            b.attach_data(data, &entity.data);
        }
        Sys::Destroy { entity } => {
            b.u32(DPSYS_DESTROYPLAYERORGROUP)
                .u32(player_type(entity))
                .u32(entity.id);
            let local = b.pointer();
            b.u32(entity.local_data.len() as u32);
            let remote = b.pointer();
            b.u32(entity.data.len() as u32);
            let n = name(&mut b);
            b.u32(entity.parent).u32(entity.reported_flags());
            n.finish(&mut b, &entity.short, &entity.long, wide);
            b.attach_data(local, &entity.local_data);
            b.attach_data(remote, &entity.data);
        }
        Sys::AddToGroup { group, player } => {
            b.u32(DPSYS_ADDPLAYERTOGROUP).u32(*group).u32(*player);
        }
        Sys::DeleteFromGroup { group, player } => {
            b.u32(DPSYS_DELETEPLAYERFROMGROUP).u32(*group).u32(*player);
        }
        Sys::AddGroupToGroup { parent, group } => {
            b.u32(DPSYS_ADDGROUPTOGROUP).u32(*parent).u32(*group);
        }
        Sys::DeleteGroupFromGroup { parent, group } => {
            b.u32(DPSYS_DELETEGROUPFROMGROUP).u32(*parent).u32(*group);
        }
        Sys::SetData { entity } => {
            b.u32(DPSYS_SETPLAYERORGROUPDATA)
                .u32(player_type(entity))
                .u32(entity.id);
            let data = b.pointer();
            b.u32(entity.data.len() as u32);
            b.attach_data(data, &entity.data);
        }
        Sys::SetName { entity } => {
            b.u32(DPSYS_SETPLAYERORGROUPNAME)
                .u32(player_type(entity))
                .u32(entity.id);
            let n = name(&mut b);
            n.finish(&mut b, &entity.short, &entity.long, wide);
        }
        Sys::SetDesc { info, password } => {
            b.u32(DPSYS_SETSESSIONDESC);
            desc2(&mut b, info, password, wide);
        }
        Sys::Host {} => {
            b.u32(DPSYS_HOST);
        }
        Sys::SessionLost {} => {
            b.u32(DPSYS_SESSIONLOST);
        }
        Sys::Chat {
            from,
            to_player,
            to_group,
            flags,
            text,
        } => {
            b.u32(DPSYS_CHAT)
                .u32(*flags)
                .u32(*from)
                .u32(*to_player)
                .u32(*to_group);
            let chat = b.pointer();
            let mut inner = Blob::default();
            inner.u32(12).u32(*flags);
            let at = inner.pointer();
            // DPCHAT and its string go after the message, as one piece
            // whose pointer is fixed up by its place.
            let start = b.bytes.len();
            b.attach(chat, &inner.bytes);
            b.attach_str(start + at, &Some(text.clone()), wide);
        }
        Sys::SetGroupOwner { group, new, old } => {
            b.u32(DPSYS_SETGROUPOWNER).u32(*group).u32(*new).u32(*old);
        }
        Sys::SendComplete {
            from,
            to,
            flags,
            priority,
            timeout,
            context,
            id,
            time,
        } => {
            b.u32(DPSYS_SENDCOMPLETE)
                .u32(*from)
                .u32(*to)
                .u32(*flags)
                .u32(*priority)
                .u32(*timeout)
                .u32(*context)
                .u32(*id)
                .u32(0)
                .u32(*time);
        }
    }
    b
}

/// A system message as IDirectPlay writes it, or None for one it
/// doesn't have, which its programs never see.
pub fn system1(sys: &Sys) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    match sys {
        Sys::Create { entity, current } => {
            put(DPSYS_CREATEPLAYERORGROUP);
            put(player_type(entity));
            put(entity.id);
            let fixed = |text: &Option<Vec<u16>>, len: usize| {
                let mut bytes = wide_to_ansi(text.as_deref().unwrap_or(&[]));
                bytes.truncate(len - 1);
                bytes.resize(len, 0);
                bytes
            };
            out.extend(fixed(&entity.long, LONG_NAME));
            out.extend(fixed(&entity.short, SHORT_NAME));
            out.extend_from_slice(&current.to_le_bytes());
        }
        Sys::Destroy { entity } => {
            put(if entity.group {
                DPSYS_DELETEGROUP
            } else {
                DPSYS_DESTROYPLAYERORGROUP
            });
            put(entity.id);
        }
        Sys::AddToGroup { group, player } => {
            put(DPSYS_ADDPLAYERTOGROUP);
            put(*group);
            put(*player);
        }
        Sys::DeleteFromGroup { group, player } => {
            put(DPSYS_DELETEPLAYERFROMGROUP);
            put(*group);
            put(*player);
        }
        Sys::SessionLost {} => put(DPSYS_SESSIONLOST),
        _ => return None,
    }
    Some(out)
}

/// IDirectPlay's player names: a fixed-size field's worth of ANSI.
pub fn name1(text: &Option<Vec<u16>>) -> Vec<u8> {
    let mut bytes = wide_to_ansi(text.as_deref().unwrap_or(&[]));
    bytes.push(0);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_messages_have_windows_sizes() {
        let bare = Entity::default();
        let named = Entity {
            short: Some("player_name".encode_utf16().collect()),
            long: Some("player_long_name".encode_utf16().collect()),
            ..Entity::default()
        };
        let size = |sys: Sys| system2(&sys, false).len();
        // The sizes Wine's tests recorded on Windows.
        assert_eq!(
            size(Sys::Create {
                entity: bare.clone(),
                current: 0
            }),
            48
        );
        assert_eq!(
            size(Sys::Destroy {
                entity: bare.clone()
            }),
            52
        );
        assert_eq!(size(Sys::SetName { entity: bare }), 28);
        assert_eq!(size(Sys::SetName { entity: named }), 57);
        let info = Info {
            name: Some("Wahaa".encode_utf16().collect()),
            ..Info::default()
        };
        let password = Vec::new();
        assert_eq!(size(Sys::SetDesc { info, password }), 90);
        assert_eq!(size(Sys::Host {}), 4);
        // A block's pointers point into it, wherever it goes.
        let entity = Entity {
            data: vec![1, 2, 3],
            short: Some(vec![65]),
            ..Entity::default()
        };
        let blob = system2(&Sys::Create { entity, current: 1 }, true);
        let at = 0x1000;
        let bytes = blob.placed(at);
        let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        assert_eq!(word(16), at + 52, "the data after the name");
        assert_eq!(word(32), at + 48, "the short name after the message");
        assert_eq!(word(36), 0, "no long name");
        assert_eq!(&bytes[48..52], &[65, 0, 0, 0]);
        assert_eq!(system1(&Sys::Host {}), None);
        assert_eq!(desc1(&Info::default()).len(), DESC1_SIZE as usize);
    }
}
