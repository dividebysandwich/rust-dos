//! TAPI32.DLL: the Telephony API of a machine with no modem installed.
//! Games of the time that play over a modem import it whole, and most of
//! them call it only when the player picks modem play, so a missing DLL
//! kept them from starting at all: Chaos Overlords links TAPICOMM, the
//! TAPI sample of the Win32 SDK, into its modem game.
//!
//! Windows 95 and 98 have no line devices until a modem is installed in
//! the Control Panel, and their lineInitialize then fails with
//! LINEERR_NODEVICE, the error TAPICOMM answers by offering to run
//! "CONTROL.EXE MODEM.CPL,,ADD". So no application gets a line
//! application handle, and every function that takes one, or a line or
//! a call opened through one, fails as for a handle that isn't one. The
//! functions that take a device ID alone fail with LINEERR_BADDEVICEID,
//! there being no device of any number.

use super::kernel32::virtual_memory::allows;
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "TAPI32.DLL",
    base: 0xBF60_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("lineAccept", 18, 3, bad_call),
    stdcall("lineAnswer", 24, 3, bad_call),
    stdcall("lineClose", 28, 1, bad_line),
    stdcall("lineConfigDialogEdit", 33, 6, bad_device),
    stdcall("lineDeallocateCall", 38, 1, bad_call),
    stdcall("lineDial", 41, 3, bad_call),
    stdcall("lineDrop", 44, 3, bad_call),
    stdcall("lineGetAddressCaps", 55, 6, bad_app),
    stdcall("lineGetAddressStatus", 61, 3, bad_line),
    stdcall("lineGetCallStatus", 78, 2, bad_call),
    stdcall("lineGetCountry", 80, 3, line_get_country),
    stdcall("lineGetDevCaps", 83, 5, bad_app),
    stdcall("lineGetDevConfig", 86, 3, bad_device),
    stdcall("lineGetID", 89, 6, line_get_id),
    stdcall("lineGetIcon", 92, 3, bad_device),
    stdcall("lineGetTranslateCaps", 108, 3, location),
    stdcall("lineInitialize", 115, 5, line_initialize),
    stdcall("lineMakeCall", 118, 5, bad_line),
    stdcall("lineNegotiateAPIVersion", 124, 6, bad_app),
    stdcall("lineOpen", 126, 9, bad_app),
    stdcall("lineSetCurrentLocation", 161, 2, location),
    stdcall("lineSetDevConfig", 162, 4, bad_device),
    stdcall("lineSetStatusMessages", 169, 3, bad_line),
    stdcall("lineShutdown", 180, 1, bad_app),
    stdcall("lineTranslateAddress", 182, 7, translate),
    stdcall("lineTranslateDialog", 185, 5, translate),
];

const LINEERR_BADDEVICEID: u32 = 0x8000_0002;
const LINEERR_INIFILECORRUPT: u32 = 0x8000_000E;
const LINEERR_INVALAPPHANDLE: u32 = 0x8000_0014;
const LINEERR_INVALCALLHANDLE: u32 = 0x8000_0018;
const LINEERR_INVALLINEHANDLE: u32 = 0x8000_002B;
const LINEERR_INVALPOINTER: u32 = 0x8000_0035;
const LINEERR_NODEVICE: u32 = 0x8000_0042;
const LINEERR_OPERATIONUNAVAIL: u32 = 0x8000_0049;

/// lineGetID's dwSelect for a call: the others name the line or one of
/// its addresses.
const LINECALLSELECT_CALL: u32 = 4;

/// lineInitialize(lphLineApp, hInstance, lpfnCallback, lpszAppName,
/// lpdwNumDevs): with no line device installed, LINEERR_NODEVICE, and
/// neither the handle nor the count is written. The places for them have
/// to be writable and the callback readable first, as IsBadWritePtr and
/// IsBadCodePtr see them, or the call fails with LINEERR_INVALPOINTER.
/// `allows` reads the pages' entries without recording a page fault, so
/// a program that passes a pointer it can't use gets the error back and
/// goes on.
fn line_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[app, _, callback, _, devices] = args else {
        unreachable!()
    };
    let room = |cpu: &mut Cpu, at: u32| at != 0 && allows(cpu, at, 4, true);
    if !room(cpu, app) || !room(cpu, devices) || callback == 0 || !allows(cpu, callback, 1, false) {
        return Some(LINEERR_INVALPOINTER);
    }
    Some(LINEERR_NODEVICE)
}

/// The functions that take a line application handle first: no
/// lineInitialize gave one.
fn bad_app(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(LINEERR_INVALAPPHANDLE)
}

/// The functions that take a line's handle first: no line was opened.
fn bad_line(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(LINEERR_INVALLINEHANDLE)
}

/// The functions that take a call's handle first: no call was made.
fn bad_call(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(LINEERR_INVALCALLHANDLE)
}

/// The functions that take a line device's ID first: there is no line
/// device of any ID.
fn bad_device(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(LINEERR_BADDEVICEID)
}

/// lineGetID(hLine, dwAddressID, hCall, dwSelect, lpDeviceID,
/// lpszDeviceClass): the call's handle is the one looked at when
/// dwSelect names the call, else the line's.
fn line_get_id(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if args[3] == LINECALLSELECT_CALL {
        LINEERR_INVALCALLHANDLE
    } else {
        LINEERR_INVALLINEHANDLE
    })
}

/// lineGetTranslateCaps(hLineApp, dwAPIVersion, lpTranslateCaps) and
/// lineSetCurrentLocation(hLineApp, dwLocation): a program that didn't
/// initialize TAPI passes a null handle, which TAPI takes, and gets
/// LINEERR_INIFILECORRUPT, the error for dialing locations that were
/// never set up; Windows sets the first one up with the first modem.
/// Any other handle isn't one.
fn location(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if args[0] == 0 {
        LINEERR_INIFILECORRUPT
    } else {
        LINEERR_INVALAPPHANDLE
    })
}

/// lineTranslateAddress(hLineApp, dwDeviceID, ...) and
/// lineTranslateDialog(hLineApp, dwDeviceID, ...): with a null handle,
/// which TAPI takes from a program that didn't initialize it, the device
/// ID is looked at, and none is one; any other handle isn't one.
fn translate(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if args[0] == 0 {
        LINEERR_BADDEVICEID
    } else {
        LINEERR_INVALAPPHANDLE
    })
}

/// lineGetCountry(dwCountryID, dwAPIVersion, lpLineCountryList): Windows
/// gives the country list from the registry, modem or not, and takes no
/// line application handle, so a program may ask for it before it
/// initializes TAPI. The built-in Windows has no country table yet, and
/// fails the call (issue #664). Chaos Overlords asks only from TAPICOMM's
/// dialing dialog, which it opens once lineInitialize has succeeded, so
/// it never gets here.
fn line_get_country(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(LINEERR_OPERATIONUNAVAIL)
}
