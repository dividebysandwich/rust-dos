//! The DirectPlay lobby (IDirectPlayLobby, 2 and 3, A and W), of a
//! program no lobby launched, on a machine with no application
//! registered for one to launch. What it has to do with a lobby fails
//! as Windows' lobby does then: there are no connection settings
//! (DPERR_NOTLOBBIED), no lobby messages, and no applications to list
//! or run. Its addresses are address.rs's.

use super::{
    DP_OK, DPERR_INVALIDFLAGS, DPERR_INVALIDPARAMS, DPERR_NOTLOBBIED, DPERR_UNKNOWNAPPLICATION,
    LOBBY, LOBBY_A, new_object, object, object_of,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, CLASS_E_NOAGGREGATION, E_OUTOFMEMORY};

/// WaitForConnectionSettings' one flag.
const DPLWAIT_CANCEL: u32 = 1;

/// DirectPlayLobbyCreateA(provider, out, outer, data, size): an
/// IDirectPlayLobbyA.
pub(super) fn create_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create(cpu, args, LOBBY_A)
}

/// DirectPlayLobbyCreateW: an IDirectPlayLobby.
pub(super) fn create_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create(cpu, args, LOBBY)
}

/// DirectPlayLobbyCreate(A, W): the provider, data and size are
/// reserved and must be 0, then the outer object, as Wine checks them.
fn create(cpu: &mut Cpu, args: &[u32], vtable: usize) -> Option<u32> {
    let [provider, out, outer, data, size] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if !com::write_out(cpu, out, 0) || provider != 0 || data != 0 || size != 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    match new_object(cpu, vtable) {
        Some(this) => {
            com::write_out(cpu, out, this);
            Some(DP_OK)
        }
        None => Some(E_OUTOFMEMORY),
    }
}

/// Connect(this, flags, out, outer): a DirectPlay object connected to
/// the session the lobby launched the program for, which it didn't.
pub(super) fn connect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    connected(cpu, args[0], args[2], args[3])
}

/// ConnectEx(this, flags, iid, out, outer): Connect, for an interface.
pub(super) fn connect_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    connected(cpu, args[0], args[3], args[4])
}

fn connected(cpu: &mut Cpu, this: u32, out: u32, outer: u32) -> Option<u32> {
    object!(cpu, this);
    if !com::write_out(cpu, out, 0) || outer != 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_NOTLOBBIED)
}

/// EnumLocalApplications(this, callback, context, flags): the
/// applications registered for lobbies, of which there are none.
pub(super) fn enum_local_applications(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    if args[1] == 0 || args[3] != 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DP_OK)
}

/// GetConnectionSettings(this, app, data, size), ReceiveLobbyMessage,
/// SendLobbyMessage and SetLobbyMessageEvent: the program wasn't
/// launched by a lobby, nor did it launch an application with one, so
/// no application of any ID is lobbied.
pub(super) fn not_lobbied(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DPERR_NOTLOBBIED)
}

/// SetConnectionSettings(this, flags, app, connection): as
/// GetConnectionSettings, given a connection.
pub(super) fn set_connection_settings(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    if args[3] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_NOTLOBBIED)
}

/// RunApplication(this, flags, app out, connection, event): the
/// application the connection's session is of isn't registered, as no
/// application is.
pub(super) fn run_application(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, app, connection, _] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    if flags != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if app == 0 || connection == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_UNKNOWNAPPLICATION)
}

/// RegisterApplication(this, flags, description): accepted, as Windows
/// and Wine accept it, though nothing keeps it: EnumLocalApplications
/// lists no applications, and RunApplication runs none.
pub(super) fn register_application(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    if args[1] != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if args[2] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DP_OK)
}

/// UnregisterApplication(this, flags, guid): as RegisterApplication.
pub(super) fn unregister_application(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    if args[1] != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if args[2] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DP_OK)
}

/// WaitForConnectionSettings(this, flags): the program waits for
/// settings no lobby sends, or stops waiting.
pub(super) fn wait_for_connection_settings(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    if args[1] & !DPLWAIT_CANCEL != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    Some(DP_OK)
}
