//! IDirectSound: the DirectSound object, which DirectSoundCreate makes,
//! or CoCreateInstance and then Initialize; its buffers; and
//! DirectSoundEnumerate, which names the device.

use super::buffer::{self, DSBCAPS_PRIMARYBUFFER};
use super::device::{Format, WAVE_FORMAT_PCM};
use super::*;
use crate::win32::task::{self, Next, Task};

/// The cooperative levels.
pub const DSSCL_NORMAL: u32 = 1;
pub const DSSCL_PRIORITY: u32 = 2;
pub const DSSCL_WRITEPRIMARY: u32 = 4;

/// The device's GUID, which DirectSoundEnumerate gives, and the default
/// playback devices' of DirectX 8, which DirectSoundCreate takes too (the
/// capture ones, DEF00001 and DEF00003, it fails with DSERR_NODRIVER).
pub const DEVICE: Guid = Guid::new(
    0x8E2D_9C40,
    0x1B57,
    0x11D2,
    [0x9C, 0x5A, 0x52, 0x41, 0x53, 0x54, 0x5D, 0x00],
);
const fn default_device(d1: u32) -> Guid {
    Guid::new(
        d1,
        0x9C6D,
        0x47ED,
        [0xAA, 0xF1, 0x4D, 0xDA, 0x8F, 0x2B, 0x5C, 0x03],
    )
}
pub const DSDEVID_DEFAULTPLAYBACK: Guid = default_device(0xDEF0_0000);
pub const DSDEVID_DEFAULTVOICEPLAYBACK: Guid = default_device(0xDEF0_0002);

/// The device's name and its driver's, as DirectSoundEnumerate gives them.
const DESCRIPTION: &str = "Built-in Windows sound";
const MODULE: &str = "DSOUND.VXD";

/// The speaker configuration when none is set: stereo, wide.
pub const DEFAULT_SPEAKERS: u32 = 4 | 20 << 16;

/// Whether the GUID at `at` (NULL for the default) names the playback
/// device: Ok, or the HRESULT to fail with.
pub fn check_device(cpu: &mut Cpu, at: u32) -> Result<(), u32> {
    if at == 0 {
        return Ok(());
    }
    let guid = Guid::read(cpu, at).ok_or(DSERR_INVALIDPARAM)?;
    if [
        DEVICE,
        com::GUID_NULL,
        DSDEVID_DEFAULTPLAYBACK,
        DSDEVID_DEFAULTVOICEPLAYBACK,
    ]
    .contains(&guid)
    {
        Ok(())
    } else {
        Err(DSERR_NODRIVER)
    }
}

/// A new DirectSound object of the running process, with a device if
/// `initialized`: its id and IDirectSound. None if the heap is full.
fn new_sound(cpu: &mut Cpu, initialized: bool) -> Option<(u32, u32)> {
    let id = state(cpu).next_id();
    let iface = new_interface(cpu, SOUND, id)?;
    let pid = crate::win32::user32::current_pid(cpu);
    state(cpu).sounds.push(Sound {
        id,
        pid,
        iface,
        refs: 1,
        initialized,
        listener: ds3d::Listener::default(),
        ..Sound::default()
    });
    Some((id, iface))
}

/// DirectSoundCreate(guid, out, outer).
pub fn direct_sound_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [guid, out, outer] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    if out == 0 || !com::write_out(cpu, out, 0) {
        return Some(DSERR_INVALIDPARAM);
    }
    if outer != 0 {
        return Some(DSERR_NOAGGREGATION);
    }
    if let Err(e) = check_device(cpu, guid) {
        return Some(e);
    }
    let Some((_, iface)) = new_sound(cpu, true) else {
        return Some(com::E_OUTOFMEMORY);
    };
    com::write_out(cpu, out, iface);
    Some(DS_OK)
}

/// CLSID_DirectSound's objects, for CoCreateInstance: without a device
/// until Initialize.
pub fn create_instance(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    if *iid != com::IID_IUNKNOWN && *iid != IID_IDIRECTSOUND {
        return com::E_NOINTERFACE;
    }
    let Some((_, iface)) = new_sound(cpu, false) else {
        return com::E_OUTOFMEMORY;
    };
    com::write_out(cpu, out, iface);
    DS_OK
}

/// The object's index, and whether it has a device.
fn sound_of(cpu: &mut Cpu, this: u32) -> Option<usize> {
    match interface(cpu, this)? {
        (SOUND, id) => state(cpu).sound(id),
        _ => None,
    }
}

/// QueryInterface on IDirectSound: IUnknown and IDirectSound (not
/// IDirectSound8, which is DirectX 8's).
pub fn query(cpu: &mut Cpu, id: u32, iid: &Guid, out: u32) -> u32 {
    let ds = state(cpu);
    let Some(i) = ds.sound(id) else {
        return DSERR_INVALIDPARAM;
    };
    if *iid != com::IID_IUNKNOWN && *iid != IID_IDIRECTSOUND {
        return com::E_NOINTERFACE;
    }
    ds.sounds[i].refs += 1;
    let iface = ds.sounds[i].iface;
    com::write_out(cpu, out, iface);
    DS_OK
}

/// Release on IDirectSound: when its count is 0 the object goes, and its
/// buffers with it, whatever their counts.
pub fn release(cpu: &mut Cpu, id: u32) -> u32 {
    let ds = state(cpu);
    let Some(i) = ds.sound(id) else {
        return 0;
    };
    ds.sounds[i].refs = ds.sounds[i].refs.saturating_sub(1);
    if ds.sounds[i].refs > 0 {
        return ds.sounds[i].refs;
    }
    let iface = ds.sounds[i].iface;
    let buffers: Vec<u32> = ds
        .buffers
        .iter()
        .filter(|b| b.sound == id)
        .map(|b| b.id)
        .collect();
    for b in buffers {
        buffer::destroy(cpu, b);
    }
    state(cpu).sounds.retain(|s| s.id != id);
    com::free_interface(cpu, iface);
    0
}

/// IDirectSound::Initialize(this, guid).
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = sound_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if state(cpu).sounds[i].initialized {
        return Some(DSERR_ALREADYINITIALIZED);
    }
    if let Err(e) = check_device(cpu, args[1]) {
        return Some(e);
    }
    state(cpu).sounds[i].initialized = true;
    Some(DS_OK)
}

/// The object, if it has a device; else the HRESULT to fail with.
fn initialized(cpu: &mut Cpu, this: u32) -> Result<usize, u32> {
    let i = sound_of(cpu, this).ok_or(DSERR_INVALIDPARAM)?;
    if !state(cpu).sounds[i].initialized {
        return Err(DSERR_UNINITIALIZED);
    }
    Ok(i)
}

/// IDirectSound::SetCooperativeLevel(this, hwnd, level).
pub fn set_cooperative_level(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let i = match initialized(cpu, args[0]) {
        Ok(i) => i,
        Err(e) => return Some(e),
    };
    let level = args[2];
    if !(DSSCL_NORMAL..=DSSCL_WRITEPRIMARY).contains(&level) {
        return Some(DSERR_INVALIDPARAM);
    }
    device::catch_up(cpu);
    let sound = &mut state(cpu).sounds[i];
    sound.level = level;
    sound.hwnd = args[1];
    Some(DS_OK)
}

/// IDirectSound::Compact(this): there is no memory on a card to compact,
/// only the level to check.
pub fn compact(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let i = match initialized(cpu, args[0]) {
        Ok(i) => i,
        Err(e) => return Some(e),
    };
    Some(if state(cpu).sounds[i].level < DSSCL_PRIORITY {
        DSERR_PRIOLEVELNEEDED
    } else {
        DS_OK
    })
}

/// IDirectSound::GetSpeakerConfig(this, out).
pub fn get_speaker_config(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Err(e) = initialized(cpu, args[0]) {
        return Some(e);
    }
    let speakers = match state(cpu).speakers {
        0 => DEFAULT_SPEAKERS,
        s => s,
    };
    Some(if com::write_out(cpu, args[1], speakers) {
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    })
}

/// IDirectSound::SetSpeakerConfig(this, config): kept for the device, as
/// Windows keeps it for the system.
pub fn set_speaker_config(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Err(e) = initialized(cpu, args[0]) {
        return Some(e);
    }
    // DSSPEAKER_HEADPHONE (1) to DSSPEAKER_SURROUND (5).
    if !(1..=5).contains(&(args[1] & 0xFF)) {
        return Some(DSERR_INVALIDPARAM);
    }
    state(cpu).speakers = args[1];
    Some(DS_OK)
}

/// DSCAPS's flags: the primary buffer in all four PCM formats, secondary
/// buffers in them at any rate (DSCAPS_CONTINUOUSRATE).
const DSCAPS_FLAGS: u32 = 0x0F1F;
const DSCAPS_SIZE: u32 = 96;

/// IDirectSound::GetCaps(this, caps): a device that mixes in software.
pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Err(e) = initialized(cpu, args[0]) {
        return Some(e);
    }
    let caps = args[1];
    if caps == 0 || !cpu.bus.guest_probe(caps, DSCAPS_SIZE as usize, true) {
        return Some(DSERR_INVALIDPARAM);
    }
    if cpu.bus.guest_read_32(caps) != DSCAPS_SIZE {
        return Some(DSERR_INVALIDPARAM);
    }
    let mut fields = [0u32; 24];
    fields[0] = DSCAPS_SIZE;
    fields[1] = DSCAPS_FLAGS;
    fields[2] = buffer::FREQUENCY_MIN;
    fields[3] = buffer::FREQUENCY_MAX;
    fields[4] = 1; // dwPrimaryBuffers
    for (n, value) in fields.iter().enumerate() {
        cpu.bus.guest_write_32(caps + 4 * n as u32, *value);
    }
    Some(DS_OK)
}

/// DSBUFFERDESC's sizes: DirectX 5's, and DirectX 7's with the 3D
/// algorithm's GUID.
const DSBUFFERDESC_SIZES: [u32; 2] = [20, 36];

/// IDirectSound::CreateSoundBuffer(this, desc, out, outer).
pub fn create_sound_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc, out, outer] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let Some(i) = sound_of(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if out == 0 || !com::write_out(cpu, out, 0) {
        return Some(DSERR_INVALIDPARAM);
    }
    if desc == 0 || !cpu.bus.guest_probe(desc, 20, false) {
        return Some(DSERR_INVALIDPARAM);
    }
    let size = cpu.bus.guest_read_32(desc);
    let flags = cpu.bus.guest_read_32(desc + 4);
    let bytes = cpu.bus.guest_read_32(desc + 8);
    let format = cpu.bus.guest_read_32(desc + 16);
    if !DSBUFFERDESC_SIZES.contains(&size) {
        return Some(DSERR_INVALIDPARAM);
    }
    if outer != 0 {
        return Some(DSERR_NOAGGREGATION);
    }
    if !state(cpu).sounds[i].initialized {
        return Some(DSERR_UNINITIALIZED);
    }
    let sound = state(cpu).sounds[i].id;
    let made = if flags & DSBCAPS_PRIMARYBUFFER != 0 {
        if format != 0 || bytes != 0 {
            return Some(DSERR_INVALIDPARAM);
        }
        buffer::primary(cpu, sound, flags)
    } else {
        if format == 0 {
            return Some(DSERR_INVALIDPARAM);
        }
        let Some((format, align)) = Format::read(cpu, format) else {
            return Some(DSERR_INVALIDPARAM);
        };
        if let Err(e) = check_format(&format, align) {
            return Some(e);
        }
        if !(buffer::SIZE_MIN..=buffer::SIZE_MAX).contains(&bytes) {
            return Some(DSERR_INVALIDPARAM);
        }
        buffer::secondary(cpu, sound, flags, format, bytes)
    };
    match made {
        Ok(iface) => {
            com::write_out(cpu, out, iface);
            Some(DS_OK)
        }
        Err(e) => Some(e),
    }
}

/// Whether a secondary buffer can have the format `format`: Ok, or the
/// HRESULT DirectX 5 and 6 fail with.
pub fn check_format(format: &Format, _align: u16) -> Result<(), u32> {
    if format.tag != WAVE_FORMAT_PCM {
        return Err(DSERR_BADFORMAT);
    }
    if !matches!(format.bits, 8 | 16) || !matches!(format.channels, 1 | 2) {
        return Err(DSERR_INVALIDPARAM);
    }
    if !(buffer::FREQUENCY_MIN..=buffer::FREQUENCY_MAX).contains(&format.rate) {
        return Err(DSERR_INVALIDPARAM);
    }
    Ok(())
}

/// IDirectSound::DuplicateSoundBuffer(this, original, out): a buffer of
/// the same samples, stopped at their start.
pub fn duplicate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, original, out] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let i = match initialized(cpu, this) {
        Ok(i) => i,
        Err(e) => return Some(e),
    };
    if out == 0 || !com::write_out(cpu, out, 0) || original == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    let sound = state(cpu).sounds[i].id;
    Some(match buffer::duplicate(cpu, sound, original) {
        Ok(iface) => {
            com::write_out(cpu, out, iface);
            DS_OK
        }
        Err(e) => e,
    })
}

/// Whether DirectSound objects of the process `pid` have flags `flags`
/// on buffers: used for sticky focus.
pub fn has_sound(ds: &State, pid: u32) -> bool {
    ds.sounds.iter().any(|s| s.pid == pid && s.level != 0)
}

/// Offsets in the enumeration's scratch space: the GUID, the
/// description and the module's name.
const ENUM_GUID: u32 = 0;
const ENUM_DESCRIPTION: u32 = 16;
const ENUM_MODULE: u32 = 176;
const ENUM_SCRATCH: u32 = 256;

/// The enumeration's variables: the callback, its context, and whether
/// it takes wide strings.
const E_CALLBACK: usize = 0;
const E_CONTEXT: usize = 1;
const E_WIDE: usize = 2;

/// DirectSoundEnumerateA(callback, context).
pub fn enumerate_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    enumerate(cpu, args, false)
}

/// DirectSoundEnumerateW(callback, context).
pub fn enumerate_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    enumerate(cpu, args, true)
}

fn enumerate(cpu: &mut Cpu, args: &[u32], wide: bool) -> Option<u32> {
    if args[0] == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    let vars = vec![args[0], args[1], wide as u32];
    task::start(cpu, task::DS_ENUMERATE, vars, ENUM_SCRATCH)
}

/// Write `text` at `at`, as the enumeration's strings are.
fn write_text(cpu: &mut Cpu, at: u32, text: &str, wide: bool) {
    let mut bytes = Vec::new();
    if wide {
        for unit in text.encode_utf16().chain([0]) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    } else {
        bytes.extend_from_slice(text.as_bytes());
        bytes.push(0);
    }
    cpu.bus.guest_write_bytes(at, &bytes);
}

/// DirectSoundEnumerate's steps: the callback for the primary sound
/// driver (no GUID), then for the device, unless the first returned
/// FALSE.
pub fn enum_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let wide = t.var(E_WIDE) != 0;
    let (callback, context) = (t.var(E_CALLBACK), t.var(E_CONTEXT));
    let at = t.scratch;
    match t.step {
        0 => {
            write_text(cpu, at + ENUM_DESCRIPTION, "Primary Sound Driver", wide);
            write_text(cpu, at + ENUM_MODULE, "", wide);
            let args = [0, at + ENUM_DESCRIPTION, at + ENUM_MODULE, context];
            Next::call(callback, &args, 1)
        }
        1 if value != 0 => {
            DEVICE.write(cpu, at + ENUM_GUID);
            write_text(cpu, at + ENUM_DESCRIPTION, DESCRIPTION, wide);
            write_text(cpu, at + ENUM_MODULE, MODULE, wide);
            let args = [
                at + ENUM_GUID,
                at + ENUM_DESCRIPTION,
                at + ENUM_MODULE,
                context,
            ];
            Next::call(callback, &args, 2)
        }
        _ => Next::Done(DS_OK),
    }
}
