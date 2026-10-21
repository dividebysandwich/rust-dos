//! DirectSoundCapture, as on a machine whose sound card only plays: no
//! capture device to enumerate or create, and CLSID_DirectSoundCapture's
//! objects, which CoCreateInstance makes, fail to initialize.

use super::*;

/// A DirectSoundCapture object CoCreateInstance made: only its interface
/// and reference count, as it never gets a device.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    pub id: u32,
    pub pid: u32,
    pub iface: u32,
    pub refs: u32,
}

/// DirectSoundCaptureCreate(guid, out, outer): there is no capture
/// device.
pub fn create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, out, outer] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    if out == 0 || !com::write_out(cpu, out, 0) {
        return Some(DSERR_INVALIDPARAM);
    }
    if outer != 0 {
        return Some(DSERR_NOAGGREGATION);
    }
    Some(DSERR_NODRIVER)
}

/// DirectSoundCaptureEnumerateA/W(callback, context): no devices to
/// call it for.
pub fn enumerate(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if args[0] == 0 {
        DSERR_INVALIDPARAM
    } else {
        DS_OK
    })
}

/// CLSID_DirectSoundCapture's objects, for CoCreateInstance.
pub fn create_instance(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    if *iid != com::IID_IUNKNOWN && *iid != IID_IDIRECTSOUNDCAPTURE {
        return com::E_NOINTERFACE;
    }
    let id = state(cpu).next_id();
    let Some(iface) = new_interface(cpu, CAPTURE, id) else {
        return com::E_OUTOFMEMORY;
    };
    let pid = crate::win32::user32::current_pid(cpu);
    state(cpu).captures.push(Capture {
        id,
        pid,
        iface,
        refs: 1,
    });
    com::write_out(cpu, out, iface);
    DS_OK
}

pub fn query(cpu: &mut Cpu, id: u32, iid: &Guid, out: u32) -> u32 {
    if *iid != com::IID_IUNKNOWN && *iid != IID_IDIRECTSOUNDCAPTURE {
        return com::E_NOINTERFACE;
    }
    let Some(c) = state(cpu).captures.iter_mut().find(|c| c.id == id) else {
        return DSERR_INVALIDPARAM;
    };
    c.refs += 1;
    let iface = c.iface;
    com::write_out(cpu, out, iface);
    DS_OK
}

pub fn add_ref(ds: &mut State, id: u32) -> u32 {
    ds.captures.iter_mut().find(|c| c.id == id).map_or(0, |c| {
        c.refs += 1;
        c.refs
    })
}

pub fn release(cpu: &mut Cpu, id: u32) -> u32 {
    let ds = state(cpu);
    let Some(i) = ds.captures.iter().position(|c| c.id == id) else {
        return 0;
    };
    ds.captures[i].refs = ds.captures[i].refs.saturating_sub(1);
    let left = ds.captures[i].refs;
    if left == 0 {
        let iface = ds.captures.remove(i).iface;
        com::free_interface(cpu, iface);
    }
    left
}

/// IDirectSoundCapture::CreateCaptureBuffer(this, desc, out, outer).
pub fn create_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[2] == 0 || !com::write_out(cpu, args[2], 0) {
        return Some(DSERR_INVALIDPARAM);
    }
    Some(DSERR_UNINITIALIZED)
}

/// IDirectSoundCapture::GetCaps(this, caps).
pub fn get_caps(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DSERR_UNINITIALIZED)
}

/// IDirectSoundCapture::Initialize(this, guid): no device to take.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DSERR_NODRIVER)
}

crate::state_fields!(Capture {
    id,
    pid,
    iface,
    refs
});
