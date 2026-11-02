//! DirectMusic, as DirectX 6.1 has it: DMUSIC.DLL (IDirectMusic, its
//! ports, buffers, clock and DLS collections, core.rs), DMIME.DLL (the
//! performance, segments and their states, tracks and graphs,
//! performance.rs and segment.rs), DMLOADER.DLL (the loader, loader.rs),
//! DMBAND.DLL (bands), DMSTYLE.DLL (styles) and DMCOMPOS.DLL (chord maps
//! and the composer, content.rs). Programs make their objects with
//! CoCreateInstance (ole32.rs) or the DLLs' DllGetClassObject.
//!
//! The music plays on the MIDI synthesizer (the MPU-401, winmm/midi.rs),
//! which both ports DirectMusic lists go to: the Microsoft Synthesizer,
//! the default one, and the MIDI mapper. Downloads of DLS instruments are
//! accepted and the synthesizer's own General MIDI instruments play
//! them, by their patches. Segments are read from DirectMusic's files
//! (.sgt) and from MIDI files (files.rs), with their sequence, tempo,
//! time signature, system exclusive and band tracks; tracks that need
//! the composition engine (styles, chords, commands) are kept and play
//! nothing, and the composer makes no segments.
//!
//! The performance plays by the emulated clock: the master clock is the
//! emulated time in 100-nanosecond units, and the process's multimedia
//! thread (winmm/callback.rs) sends each event at its time (`poll`).
//! Tools put in graphs are kept and not called.
//!
//! Each object is in `State::objects`, with one reference count for all
//! its interfaces, as DirectMusic's objects have. Objects that hold
//! others (a segment its tracks, a performance its ports) hold
//! references on them.

mod content;
mod core;
mod files;
pub mod guids;
mod loader;
mod performance;
pub mod riff;
mod segment;

use super::com::{self, Guid};
use super::stubs::{Api, Conv, Dll, Handler, Vtable};
use crate::cpu::Cpu;
use guids::*;

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// A method of an interface (not exported).
const fn method(name: &'static str, args: u8, handler: Handler) -> Api {
    stdcall(name, 0, args, handler)
}

pub static DMUSIC: Dll = Dll {
    name: "DMUSIC.DLL",
    base: 0xBFD4_0000,
    apis: DMUSIC_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: DMUSIC_VTABLES,
};

pub static DMIME: Dll = Dll {
    name: "DMIME.DLL",
    base: 0xBFD6_0000,
    apis: DMIME_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: DMIME_VTABLES,
};

pub static DMLOADER: Dll = Dll {
    name: "DMLOADER.DLL",
    base: 0xBFD8_0000,
    apis: DMLOADER_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: DMLOADER_VTABLES,
};

pub static DMSTYLE: Dll = Dll {
    name: "DMSTYLE.DLL",
    base: 0xBFDC_0000,
    apis: DMSTYLE_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: DMSTYLE_VTABLES,
};

pub static DMBAND: Dll = Dll {
    name: "DMBAND.DLL",
    base: 0xBFDE_0000,
    apis: DMBAND_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: DMBAND_VTABLES,
};

pub static DMCOMPOS: Dll = Dll {
    name: "DMCOMPOS.DLL",
    base: 0xBFE0_0000,
    apis: DMCOMPOS_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: DMCOMPOS_VTABLES,
};

/// The DLLs, by their places in `Iface::dll`.
static DLLS: [&Dll; 6] = [&DMUSIC, &DMIME, &DMLOADER, &DMSTYLE, &DMBAND, &DMCOMPOS];
const D_DMUSIC: u8 = 0;
const D_DMIME: u8 = 1;
const D_DMLOADER: u8 = 2;
const D_DMSTYLE: u8 = 3;
const D_DMBAND: u8 = 4;
const D_DMCOMPOS: u8 = 5;

/// IDirectMusicObject's and IPersistStream's methods, which several DLLs
/// have.
const OBJECT_METHODS: &[&str] = &[
    "QueryInterface",
    "AddRef",
    "Release",
    "Object_GetDescriptor",
    "Object_SetDescriptor",
    "Object_ParseDescriptor",
];
const PERSIST_METHODS: &[&str] = &[
    "QueryInterface",
    "AddRef",
    "Release",
    "Persist_GetClassID",
    "Persist_IsDirty",
    "Persist_Load",
    "Persist_Save",
    "Persist_GetSizeMax",
];

/// DMUSIC.DLL's functions. Append only (see `stubs`).
static DMUSIC_APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object_dmusic),
    stdcall("DllRegisterServer", 3, 0, s_ok),
    stdcall("DllUnregisterServer", 4, 0, s_ok),
    method("QueryInterface", 3, query_interface),
    method("AddRef", 1, add_ref),
    method("Release", 1, release),
    method("Object_GetDescriptor", 2, get_descriptor),
    method("Object_SetDescriptor", 2, set_descriptor),
    method("Object_ParseDescriptor", 3, parse_descriptor),
    method("Persist_GetClassID", 2, get_class_id),
    method("Persist_IsDirty", 1, s_false),
    method("Persist_Load", 2, persist_load),
    method("Persist_Save", 3, e_notimpl),
    method("Persist_GetSizeMax", 2, e_notimpl),
    method("Music_EnumPort", 3, core::enum_port),
    method("Music_CreateMusicBuffer", 4, core::create_music_buffer),
    method("Music_CreatePort", 5, core::create_port),
    method("Music_EnumMasterClock", 3, core::enum_master_clock),
    method("Music_GetMasterClock", 3, core::get_master_clock),
    method("Music_SetMasterClock", 2, core::set_master_clock),
    method("Music_Activate", 2, s_ok),
    method("Music_GetDefaultPort", 2, core::get_default_port),
    method("Music_SetDirectSound", 3, core::set_direct_sound),
    method("Music_SetExternalMasterClock", 2, e_notimpl),
    method("Port_PlayBuffer", 2, core::play_buffer),
    method("Port_SetReadNotificationHandle", 2, s_ok),
    method("Port_Read", 2, core::port_read),
    method("Port_DownloadInstrument", 5, core::download_instrument),
    method("Port_UnloadInstrument", 2, s_ok),
    method("Port_GetLatencyClock", 2, core::get_latency_clock),
    method("Port_GetRunningStats", 2, core::get_running_stats),
    method("Port_Compact", 1, s_ok),
    method("Port_GetCaps", 2, core::port_get_caps),
    method("Port_DeviceIoControl", 8, e_notimpl),
    method("Port_SetNumChannelGroups", 2, core::set_channel_groups),
    method("Port_GetNumChannelGroups", 2, core::get_channel_groups),
    method("Port_Activate", 2, core::port_activate),
    method("Port_SetChannelPriority", 4, s_ok),
    method("Port_GetChannelPriority", 4, core::get_channel_priority),
    method("Port_SetDirectSound", 3, s_ok),
    method("Port_GetFormat", 4, core::port_get_format),
    method("Buffer_Flush", 1, core::buffer_flush),
    method("Buffer_TotalTime", 2, core::buffer_total_time),
    method("Buffer_PackStructured", 5, core::pack_structured),
    method("Buffer_PackUnstructured", 6, core::pack_unstructured),
    method("Buffer_ResetReadPtr", 1, core::reset_read_ptr),
    method("Buffer_GetNextEvent", 5, core::get_next_event),
    method("Buffer_GetRawBufferPtr", 2, core::get_raw_buffer_ptr),
    method("Buffer_GetStartTime", 2, core::buffer_get_start_time),
    method("Buffer_GetUsedBytes", 2, core::get_used_bytes),
    method("Buffer_GetMaxBytes", 2, core::get_max_bytes),
    method("Buffer_GetBufferFormat", 2, core::get_buffer_format),
    method("Buffer_SetStartTime", 3, core::buffer_set_start_time),
    method("Buffer_SetUsedBytes", 2, core::set_used_bytes),
    method("Clock_GetTime", 2, core::clock_get_time),
    method("Clock_AdviseTime", 7, e_notimpl),
    method("Clock_AdvisePeriodic", 7, e_notimpl),
    method("Clock_Unadvise", 2, e_notimpl),
    method("Collection_GetInstrument", 3, core::get_instrument),
    method("Collection_EnumInstrument", 5, core::enum_instrument),
    method("Instrument_GetPatch", 2, core::get_patch),
    method("Instrument_SetPatch", 2, core::set_patch),
];

static DMUSIC_VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectMusic",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Music_EnumPort",
            "Music_CreateMusicBuffer",
            "Music_CreatePort",
            "Music_EnumMasterClock",
            "Music_GetMasterClock",
            "Music_SetMasterClock",
            "Music_Activate",
            "Music_GetDefaultPort",
            "Music_SetDirectSound",
            "Music_SetExternalMasterClock",
        ],
    },
    Vtable {
        name: "IDirectMusicPort",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Port_PlayBuffer",
            "Port_SetReadNotificationHandle",
            "Port_Read",
            "Port_DownloadInstrument",
            "Port_UnloadInstrument",
            "Port_GetLatencyClock",
            "Port_GetRunningStats",
            "Port_Compact",
            "Port_GetCaps",
            "Port_DeviceIoControl",
            "Port_SetNumChannelGroups",
            "Port_GetNumChannelGroups",
            "Port_Activate",
            "Port_SetChannelPriority",
            "Port_GetChannelPriority",
            "Port_SetDirectSound",
            "Port_GetFormat",
        ],
    },
    Vtable {
        name: "IDirectMusicBuffer",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Buffer_Flush",
            "Buffer_TotalTime",
            "Buffer_PackStructured",
            "Buffer_PackUnstructured",
            "Buffer_ResetReadPtr",
            "Buffer_GetNextEvent",
            "Buffer_GetRawBufferPtr",
            "Buffer_GetStartTime",
            "Buffer_GetUsedBytes",
            "Buffer_GetMaxBytes",
            "Buffer_GetBufferFormat",
            "Buffer_SetStartTime",
            "Buffer_SetUsedBytes",
        ],
    },
    Vtable {
        name: "IReferenceClock",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Clock_GetTime",
            "Clock_AdviseTime",
            "Clock_AdvisePeriodic",
            "Clock_Unadvise",
        ],
    },
    Vtable {
        name: "IDirectMusicCollection",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Collection_GetInstrument",
            "Collection_EnumInstrument",
        ],
    },
    Vtable {
        name: "IDirectMusicInstrument",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Instrument_GetPatch",
            "Instrument_SetPatch",
        ],
    },
    Vtable {
        name: "IDirectMusicDownloadedInstrument",
        methods: &["QueryInterface", "AddRef", "Release"],
    },
    Vtable {
        name: "IDirectMusicObject",
        methods: OBJECT_METHODS,
    },
    Vtable {
        name: "IPersistStream",
        methods: PERSIST_METHODS,
    },
];

/// DMIME.DLL's functions. Append only (see `stubs`).
static DMIME_APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object_dmime),
    stdcall("DllRegisterServer", 3, 0, s_ok),
    stdcall("DllUnregisterServer", 4, 0, s_ok),
    method("QueryInterface", 3, query_interface),
    method("AddRef", 1, add_ref),
    method("Release", 1, release),
    method("Object_GetDescriptor", 2, get_descriptor),
    method("Object_SetDescriptor", 2, set_descriptor),
    method("Object_ParseDescriptor", 3, parse_descriptor),
    method("Persist_GetClassID", 2, get_class_id),
    method("Persist_IsDirty", 1, s_false),
    method("Persist_Load", 2, persist_load),
    method("Persist_Save", 3, e_notimpl),
    method("Persist_GetSizeMax", 2, e_notimpl),
    method("Performance_Init", 4, performance::init),
    method("Performance_PlaySegment", 6, performance::play_segment),
    method("Performance_Stop", 5, performance::stop),
    method(
        "Performance_GetSegmentState",
        3,
        performance::get_segment_state,
    ),
    method(
        "Performance_SetPrepareTime",
        2,
        performance::set_prepare_time,
    ),
    method(
        "Performance_GetPrepareTime",
        2,
        performance::get_prepare_time,
    ),
    method(
        "Performance_SetBumperLength",
        2,
        performance::set_bumper_length,
    ),
    method(
        "Performance_GetBumperLength",
        2,
        performance::get_bumper_length,
    ),
    method("Performance_SendPMsg", 2, performance::send_pmsg),
    method(
        "Performance_MusicToReferenceTime",
        3,
        performance::music_to_reference_time,
    ),
    method(
        "Performance_ReferenceToMusicTime",
        4,
        performance::reference_to_music_time,
    ),
    method("Performance_IsPlaying", 3, performance::is_playing),
    method("Performance_GetTime", 3, performance::get_time),
    method("Performance_AllocPMsg", 3, performance::alloc_pmsg),
    method("Performance_FreePMsg", 2, performance::free_pmsg),
    method("Performance_GetGraph", 2, segment::get_graph),
    method("Performance_SetGraph", 2, segment::set_graph),
    method(
        "Performance_SetNotificationHandle",
        4,
        performance::set_notification_handle,
    ),
    method(
        "Performance_GetNotificationPMsg",
        2,
        performance::get_notification_pmsg,
    ),
    method(
        "Performance_AddNotificationType",
        2,
        performance::add_notification_type,
    ),
    method(
        "Performance_RemoveNotificationType",
        2,
        performance::remove_notification_type,
    ),
    method("Performance_AddPort", 2, performance::add_port),
    method("Performance_RemovePort", 2, performance::remove_port),
    method(
        "Performance_AssignPChannelBlock",
        4,
        performance::assign_pchannel_block,
    ),
    method(
        "Performance_AssignPChannel",
        5,
        performance::assign_pchannel,
    ),
    method("Performance_PChannelInfo", 5, performance::pchannel_info),
    method(
        "Performance_DownloadInstrument",
        9,
        performance::download_instrument,
    ),
    method("Performance_Invalidate", 3, s_ok),
    method("Performance_GetParam", 7, performance::get_param),
    method("Performance_SetParam", 6, performance::set_param),
    method(
        "Performance_GetGlobalParam",
        4,
        performance::get_global_param,
    ),
    method(
        "Performance_SetGlobalParam",
        4,
        performance::set_global_param,
    ),
    method(
        "Performance_GetLatencyTime",
        2,
        performance::get_latency_time,
    ),
    method("Performance_GetQueueTime", 2, performance::get_latency_time),
    method("Performance_AdjustTime", 3, s_ok),
    method("Performance_CloseDown", 1, performance::close_down),
    method(
        "Performance_GetResolvedTime",
        5,
        performance::get_resolved_time,
    ),
    method("Performance_MIDIToMusic", 6, e_notimpl),
    method("Performance_MusicToMIDI", 6, e_notimpl),
    method("Performance_TimeToRhythm", 7, performance::time_to_rhythm),
    method("Performance_RhythmToTime", 7, performance::rhythm_to_time),
    method("Segment_GetLength", 2, segment::get_length),
    method("Segment_SetLength", 2, segment::set_length),
    method("Segment_GetRepeats", 2, segment::get_repeats),
    method("Segment_SetRepeats", 2, segment::set_repeats),
    method(
        "Segment_GetDefaultResolution",
        2,
        segment::get_default_resolution,
    ),
    method(
        "Segment_SetDefaultResolution",
        2,
        segment::set_default_resolution,
    ),
    method("Segment_GetTrack", 5, segment::get_track),
    method("Segment_GetTrackGroup", 3, segment::get_track_group),
    method("Segment_InsertTrack", 3, segment::insert_track),
    method("Segment_RemoveTrack", 2, segment::remove_track),
    method("Segment_InitPlay", 4, e_notimpl),
    method("Segment_GetGraph", 2, segment::get_graph),
    method("Segment_SetGraph", 2, segment::set_graph),
    method(
        "Segment_AddNotificationType",
        2,
        segment::add_notification_type,
    ),
    method(
        "Segment_RemoveNotificationType",
        2,
        segment::remove_notification_type,
    ),
    method("Segment_GetParam", 7, segment::get_param),
    method("Segment_SetParam", 6, segment::set_param),
    method("Segment_Clone", 4, segment::clone),
    method("Segment_SetStartPoint", 2, segment::set_start_point),
    method("Segment_GetStartPoint", 2, segment::get_start_point),
    method("Segment_SetLoopPoints", 3, segment::set_loop_points),
    method("Segment_GetLoopPoints", 3, segment::get_loop_points),
    method("Segment_SetPChannelsUsed", 3, s_ok),
    method("SegmentState_GetRepeats", 2, performance::state_get_repeats),
    method("SegmentState_GetSegment", 2, performance::state_get_segment),
    method(
        "SegmentState_GetStartTime",
        2,
        performance::state_get_start_time,
    ),
    method("SegmentState_GetSeek", 2, performance::state_get_seek),
    method(
        "SegmentState_GetStartPoint",
        2,
        performance::state_get_start_point,
    ),
    method("Graph_StampPMsg", 2, s_ok),
    method("Graph_InsertTool", 5, segment::insert_tool),
    method("Graph_GetTool", 3, segment::get_tool),
    method("Graph_RemoveTool", 2, segment::remove_tool),
    method("Track_Init", 2, s_ok),
    method("Track_InitPlay", 6, segment::track_init_play),
    method("Track_EndPlay", 2, s_ok),
    method("Track_Play", 9, s_ok),
    method("Track_GetParam", 5, segment::track_get_param),
    method("Track_SetParam", 4, segment::track_set_param),
    method(
        "Track_IsParamSupported",
        2,
        segment::track_is_param_supported,
    ),
    method("Track_AddNotificationType", 2, s_ok),
    method("Track_RemoveNotificationType", 2, s_ok),
    method("Track_Clone", 4, segment::track_clone),
];

static DMIME_VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectMusicPerformance",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Performance_Init",
            "Performance_PlaySegment",
            "Performance_Stop",
            "Performance_GetSegmentState",
            "Performance_SetPrepareTime",
            "Performance_GetPrepareTime",
            "Performance_SetBumperLength",
            "Performance_GetBumperLength",
            "Performance_SendPMsg",
            "Performance_MusicToReferenceTime",
            "Performance_ReferenceToMusicTime",
            "Performance_IsPlaying",
            "Performance_GetTime",
            "Performance_AllocPMsg",
            "Performance_FreePMsg",
            "Performance_GetGraph",
            "Performance_SetGraph",
            "Performance_SetNotificationHandle",
            "Performance_GetNotificationPMsg",
            "Performance_AddNotificationType",
            "Performance_RemoveNotificationType",
            "Performance_AddPort",
            "Performance_RemovePort",
            "Performance_AssignPChannelBlock",
            "Performance_AssignPChannel",
            "Performance_PChannelInfo",
            "Performance_DownloadInstrument",
            "Performance_Invalidate",
            "Performance_GetParam",
            "Performance_SetParam",
            "Performance_GetGlobalParam",
            "Performance_SetGlobalParam",
            "Performance_GetLatencyTime",
            "Performance_GetQueueTime",
            "Performance_AdjustTime",
            "Performance_CloseDown",
            "Performance_GetResolvedTime",
            "Performance_MIDIToMusic",
            "Performance_MusicToMIDI",
            "Performance_TimeToRhythm",
            "Performance_RhythmToTime",
        ],
    },
    Vtable {
        name: "IDirectMusicSegment",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Segment_GetLength",
            "Segment_SetLength",
            "Segment_GetRepeats",
            "Segment_SetRepeats",
            "Segment_GetDefaultResolution",
            "Segment_SetDefaultResolution",
            "Segment_GetTrack",
            "Segment_GetTrackGroup",
            "Segment_InsertTrack",
            "Segment_RemoveTrack",
            "Segment_InitPlay",
            "Segment_GetGraph",
            "Segment_SetGraph",
            "Segment_AddNotificationType",
            "Segment_RemoveNotificationType",
            "Segment_GetParam",
            "Segment_SetParam",
            "Segment_Clone",
            "Segment_SetStartPoint",
            "Segment_GetStartPoint",
            "Segment_SetLoopPoints",
            "Segment_GetLoopPoints",
            "Segment_SetPChannelsUsed",
        ],
    },
    Vtable {
        name: "IDirectMusicSegmentState",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "SegmentState_GetRepeats",
            "SegmentState_GetSegment",
            "SegmentState_GetStartTime",
            "SegmentState_GetSeek",
            "SegmentState_GetStartPoint",
        ],
    },
    Vtable {
        name: "IDirectMusicGraph",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Graph_StampPMsg",
            "Graph_InsertTool",
            "Graph_GetTool",
            "Graph_RemoveTool",
        ],
    },
    Vtable {
        name: "IDirectMusicTrack",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Track_Init",
            "Track_InitPlay",
            "Track_EndPlay",
            "Track_Play",
            "Track_GetParam",
            "Track_SetParam",
            "Track_IsParamSupported",
            "Track_AddNotificationType",
            "Track_RemoveNotificationType",
            "Track_Clone",
        ],
    },
    Vtable {
        name: "IDirectMusicObject",
        methods: OBJECT_METHODS,
    },
    Vtable {
        name: "IPersistStream",
        methods: PERSIST_METHODS,
    },
];

/// DMLOADER.DLL's functions. Append only (see `stubs`).
static DMLOADER_APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object_dmloader),
    stdcall("DllRegisterServer", 3, 0, s_ok),
    stdcall("DllUnregisterServer", 4, 0, s_ok),
    method("QueryInterface", 3, query_interface),
    method("AddRef", 1, add_ref),
    method("Release", 1, release),
    method("Loader_GetObject", 4, loader::get_object),
    method("Loader_SetObject", 2, loader::set_object),
    method("Loader_SetSearchDirectory", 4, loader::set_search_directory),
    method("Loader_ScanDirectory", 4, loader::scan_directory),
    method("Loader_CacheObject", 2, loader::cache_object),
    method("Loader_ReleaseObject", 2, loader::release_object),
    method("Loader_ClearCache", 2, loader::clear_cache),
    method("Loader_EnableCache", 3, loader::enable_cache),
    method("Loader_EnumObject", 4, loader::enum_object),
];

static DMLOADER_VTABLES: &[Vtable] = &[Vtable {
    name: "IDirectMusicLoader",
    methods: &[
        "QueryInterface",
        "AddRef",
        "Release",
        "Loader_GetObject",
        "Loader_SetObject",
        "Loader_SetSearchDirectory",
        "Loader_ScanDirectory",
        "Loader_CacheObject",
        "Loader_ReleaseObject",
        "Loader_ClearCache",
        "Loader_EnableCache",
        "Loader_EnumObject",
    ],
}];

/// DMSTYLE.DLL's functions. Append only (see `stubs`).
static DMSTYLE_APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object_dmstyle),
    stdcall("DllRegisterServer", 3, 0, s_ok),
    stdcall("DllUnregisterServer", 4, 0, s_ok),
    method("QueryInterface", 3, query_interface),
    method("AddRef", 1, add_ref),
    method("Release", 1, release),
    method("Object_GetDescriptor", 2, get_descriptor),
    method("Object_SetDescriptor", 2, set_descriptor),
    method("Object_ParseDescriptor", 3, parse_descriptor),
    method("Persist_GetClassID", 2, get_class_id),
    method("Persist_IsDirty", 1, s_false),
    method("Persist_Load", 2, persist_load),
    method("Persist_Save", 3, e_notimpl),
    method("Persist_GetSizeMax", 2, e_notimpl),
    method("Style_GetBand", 3, content::style_get_band),
    method("Style_EnumBand", 3, content::style_enum_band),
    method("Style_GetDefaultBand", 2, content::style_get_default_band),
    method("Style_EnumMotif", 3, s_false),
    method("Style_GetMotif", 3, content::not_found),
    method("Style_GetDefaultChordMap", 2, content::not_found),
    method("Style_EnumChordMap", 3, s_false),
    method("Style_GetChordMap", 3, content::not_found),
    method(
        "Style_GetTimeSignature",
        2,
        content::style_get_time_signature,
    ),
    method("Style_GetEmbellishmentLength", 5, content::not_found),
    method("Style_GetTempo", 2, content::style_get_tempo),
];

static DMSTYLE_VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectMusicStyle",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Style_GetBand",
            "Style_EnumBand",
            "Style_GetDefaultBand",
            "Style_EnumMotif",
            "Style_GetMotif",
            "Style_GetDefaultChordMap",
            "Style_EnumChordMap",
            "Style_GetChordMap",
            "Style_GetTimeSignature",
            "Style_GetEmbellishmentLength",
            "Style_GetTempo",
        ],
    },
    Vtable {
        name: "IDirectMusicObject",
        methods: OBJECT_METHODS,
    },
    Vtable {
        name: "IPersistStream",
        methods: PERSIST_METHODS,
    },
];

/// DMBAND.DLL's functions. Append only (see `stubs`).
static DMBAND_APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object_dmband),
    stdcall("DllRegisterServer", 3, 0, s_ok),
    stdcall("DllUnregisterServer", 4, 0, s_ok),
    method("QueryInterface", 3, query_interface),
    method("AddRef", 1, add_ref),
    method("Release", 1, release),
    method("Object_GetDescriptor", 2, get_descriptor),
    method("Object_SetDescriptor", 2, set_descriptor),
    method("Object_ParseDescriptor", 3, parse_descriptor),
    method("Persist_GetClassID", 2, get_class_id),
    method("Persist_IsDirty", 1, s_false),
    method("Persist_Load", 2, persist_load),
    method("Persist_Save", 3, e_notimpl),
    method("Persist_GetSizeMax", 2, e_notimpl),
    method("Band_CreateSegment", 2, content::band_create_segment),
    method("Band_Download", 2, content::band_download),
    method("Band_Unload", 2, content::band_download),
];

static DMBAND_VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectMusicBand",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Band_CreateSegment",
            "Band_Download",
            "Band_Unload",
        ],
    },
    Vtable {
        name: "IDirectMusicObject",
        methods: OBJECT_METHODS,
    },
    Vtable {
        name: "IPersistStream",
        methods: PERSIST_METHODS,
    },
];

/// DMCOMPOS.DLL's functions. Append only (see `stubs`).
static DMCOMPOS_APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object_dmcompos),
    stdcall("DllRegisterServer", 3, 0, s_ok),
    stdcall("DllUnregisterServer", 4, 0, s_ok),
    method("QueryInterface", 3, query_interface),
    method("AddRef", 1, add_ref),
    method("Release", 1, release),
    method("Object_GetDescriptor", 2, get_descriptor),
    method("Object_SetDescriptor", 2, set_descriptor),
    method("Object_ParseDescriptor", 3, parse_descriptor),
    method("Persist_GetClassID", 2, get_class_id),
    method("Persist_IsDirty", 1, s_false),
    method("Persist_Load", 2, persist_load),
    method("Persist_Save", 3, e_notimpl),
    method("Persist_GetSizeMax", 2, e_notimpl),
    method("ChordMap_GetScale", 2, content::chord_map_get_scale),
    method("Composer_ComposeSegmentFromTemplate", 6, e_notimpl),
    method("Composer_ComposeSegmentFromShape", 9, e_notimpl),
    method("Composer_ComposeTransition", 8, e_notimpl),
    method("Composer_AutoTransition", 9, e_notimpl),
    method("Composer_ComposeTemplateFromShape", 7, e_notimpl),
    method("Composer_ChangeChordMap", 4, e_notimpl),
];

static DMCOMPOS_VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectMusicChordMap",
        methods: &["QueryInterface", "AddRef", "Release", "ChordMap_GetScale"],
    },
    Vtable {
        name: "IDirectMusicComposer",
        methods: &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Composer_ComposeSegmentFromTemplate",
            "Composer_ComposeSegmentFromShape",
            "Composer_ComposeTransition",
            "Composer_AutoTransition",
            "Composer_ComposeTemplateFromShape",
            "Composer_ChangeChordMap",
        ],
    },
    Vtable {
        name: "IDirectMusicObject",
        methods: OBJECT_METHODS,
    },
    Vtable {
        name: "IPersistStream",
        methods: PERSIST_METHODS,
    },
];

/// An interface: its DLL's place in `DLLS` and its vtable's in the DLL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Iface(u8, u8);

impl Iface {
    fn key(self) -> u32 {
        (self.0 as u32) << 8 | self.1 as u32
    }
}

pub const MUSIC: Iface = Iface(D_DMUSIC, 0);
pub const PORT: Iface = Iface(D_DMUSIC, 1);
pub const BUFFER: Iface = Iface(D_DMUSIC, 2);
pub const CLOCK: Iface = Iface(D_DMUSIC, 3);
pub const COLLECTION: Iface = Iface(D_DMUSIC, 4);
pub const INSTRUMENT: Iface = Iface(D_DMUSIC, 5);
pub const DOWNLOADED: Iface = Iface(D_DMUSIC, 6);
const DMUSIC_OBJECT: Iface = Iface(D_DMUSIC, 7);
const DMUSIC_PERSIST: Iface = Iface(D_DMUSIC, 8);
pub const PERFORMANCE: Iface = Iface(D_DMIME, 0);
pub const SEGMENT: Iface = Iface(D_DMIME, 1);
pub const SEGMENT_STATE: Iface = Iface(D_DMIME, 2);
pub const GRAPH: Iface = Iface(D_DMIME, 3);
pub const TRACK: Iface = Iface(D_DMIME, 4);
const DMIME_OBJECT: Iface = Iface(D_DMIME, 5);
const DMIME_PERSIST: Iface = Iface(D_DMIME, 6);
pub const LOADER: Iface = Iface(D_DMLOADER, 0);
pub const STYLE: Iface = Iface(D_DMSTYLE, 0);
const DMSTYLE_OBJECT: Iface = Iface(D_DMSTYLE, 1);
const DMSTYLE_PERSIST: Iface = Iface(D_DMSTYLE, 2);
pub const BAND: Iface = Iface(D_DMBAND, 0);
const DMBAND_OBJECT: Iface = Iface(D_DMBAND, 1);
const DMBAND_PERSIST: Iface = Iface(D_DMBAND, 2);
pub const CHORD_MAP: Iface = Iface(D_DMCOMPOS, 0);
pub const COMPOSER: Iface = Iface(D_DMCOMPOS, 1);
const DMCOMPOS_OBJECT: Iface = Iface(D_DMCOMPOS, 2);
const DMCOMPOS_PERSIST: Iface = Iface(D_DMCOMPOS, 3);

/// The classes CoCreateInstance makes of DirectMusic's (com.rs).
pub static CLASSES: &[com::Class] = &[
    class(
        CLSID_DIRECTMUSIC,
        "DMUSIC.DLL",
        "DirectMusic",
        core::create_music,
    ),
    class(
        CLSID_COLLECTION,
        "DMUSIC.DLL",
        "DirectMusicCollection",
        core::create_collection,
    ),
    class(
        CLSID_PERFORMANCE,
        "DMIME.DLL",
        "DirectMusicPerformance",
        performance::create,
    ),
    class(
        CLSID_SEGMENT,
        "DMIME.DLL",
        "DirectMusicSegment",
        segment::create_segment,
    ),
    class(
        CLSID_SEGMENT_STATE,
        "DMIME.DLL",
        "DirectMusicSegmentState",
        performance::create_state,
    ),
    class(
        CLSID_GRAPH,
        "DMIME.DLL",
        "DirectMusicGraph",
        segment::create_graph,
    ),
    class(
        CLSID_TEMPO_TRACK,
        "DMIME.DLL",
        "DirectMusicTempoTrack",
        segment::create_tempo_track,
    ),
    class(
        CLSID_SEQ_TRACK,
        "DMIME.DLL",
        "DirectMusicSeqTrack",
        segment::create_seq_track,
    ),
    class(
        CLSID_SYSEX_TRACK,
        "DMIME.DLL",
        "DirectMusicSysExTrack",
        segment::create_sysex_track,
    ),
    class(
        CLSID_TIME_SIG_TRACK,
        "DMIME.DLL",
        "DirectMusicTimeSigTrack",
        segment::create_time_sig_track,
    ),
    class(
        CLSID_LOADER,
        "DMLOADER.DLL",
        "DirectMusicLoader",
        loader::create,
    ),
    class(
        CLSID_STYLE,
        "DMSTYLE.DLL",
        "DirectMusicStyle",
        content::create_style,
    ),
    class(
        CLSID_CHORD_TRACK,
        "DMSTYLE.DLL",
        "DirectMusicChordTrack",
        segment::create_chord_track,
    ),
    class(
        CLSID_COMMAND_TRACK,
        "DMSTYLE.DLL",
        "DirectMusicCommandTrack",
        segment::create_command_track,
    ),
    class(
        CLSID_STYLE_TRACK,
        "DMSTYLE.DLL",
        "DirectMusicStyleTrack",
        segment::create_style_track,
    ),
    class(
        CLSID_MOTIF_TRACK,
        "DMSTYLE.DLL",
        "DirectMusicMotifTrack",
        segment::create_motif_track,
    ),
    class(
        CLSID_MUTE_TRACK,
        "DMSTYLE.DLL",
        "DirectMusicMuteTrack",
        segment::create_mute_track,
    ),
    class(
        CLSID_BAND,
        "DMBAND.DLL",
        "DirectMusicBand",
        content::create_band,
    ),
    class(
        CLSID_BAND_TRACK,
        "DMBAND.DLL",
        "DirectMusicBandTrack",
        segment::create_band_track,
    ),
    class(
        CLSID_CHORD_MAP,
        "DMCOMPOS.DLL",
        "DirectMusicChordMap",
        content::create_chord_map,
    ),
    class(
        CLSID_COMPOSER,
        "DMCOMPOS.DLL",
        "DirectMusicComposer",
        content::create_composer,
    ),
    class(
        CLSID_CHORD_MAP_TRACK,
        "DMCOMPOS.DLL",
        "DirectMusicChordMapTrack",
        segment::create_chord_map_track,
    ),
    class(
        CLSID_SIGN_POST_TRACK,
        "DMCOMPOS.DLL",
        "DirectMusicSignPostTrack",
        segment::create_sign_post_track,
    ),
];

const fn class(
    clsid: Guid,
    dll: &'static str,
    name: &'static str,
    create: fn(&mut Cpu, &Guid, u32) -> u32,
) -> com::Class {
    com::Class {
        clsid,
        dll,
        name,
        create,
    }
}

/// Whether the class `clsid` is one of DirectMusic's, which are
/// apartment-threaded: a thread in the multithreaded apartment gets them
/// only as IUnknown (ole32.rs).
pub fn is_apartment_class(clsid: &Guid) -> bool {
    CLASSES.iter().any(|c| c.clsid == *clsid)
}

/// HRESULTs.
pub const S_OK: u32 = com::S_OK;
pub const S_FALSE: u32 = com::S_FALSE;
pub const E_POINTER: u32 = com::E_POINTER;
pub const E_INVALIDARG: u32 = com::E_INVALIDARG;
pub const E_NOINTERFACE: u32 = com::E_NOINTERFACE;
pub const E_NOTIMPL: u32 = com::E_NOTIMPL;
pub const E_OUTOFMEMORY: u32 = com::E_OUTOFMEMORY;

/// DirectMusic's errors: MAKE_DMHRESULTERROR(code).
pub const fn dmus_e(code: u32) -> u32 {
    0x8878_1000 | code
}
pub const DMUS_E_NOT_FOUND: u32 = dmus_e(0x161);
pub const DMUS_E_INVALIDFILE: u32 = dmus_e(0x200);
use loader::DMUS_E_UNSUPPORTED_STREAM;

/// What IDirectMusicObject says of an object (DMUS_OBJECTDESC).
#[derive(Clone, Debug, Default)]
pub struct Desc {
    /// Its DMUS_OBJ_ flags.
    pub valid: u32,
    pub object: [u8; 16],
    pub class: [u8; 16],
    pub name: String,
    pub category: String,
    pub file: String,
    /// The version's high and low words, and the date (FILETIME).
    pub version: (u32, u32),
    pub date: u64,
}

/// DMUS_OBJ_ flags of DMUS_OBJECTDESC's dwValidData.
pub const DMUS_OBJ_OBJECT: u32 = 1;
pub const DMUS_OBJ_CLASS: u32 = 2;
pub const DMUS_OBJ_NAME: u32 = 4;
pub const DMUS_OBJ_CATEGORY: u32 = 8;
pub const DMUS_OBJ_FILENAME: u32 = 0x10;
pub const DMUS_OBJ_FULLPATH: u32 = 0x20;
pub const DMUS_OBJ_VERSION: u32 = 0x80;
pub const DMUS_OBJ_DATE: u32 = 0x100;
pub const DMUS_OBJ_LOADED: u32 = 0x200;
pub const DMUS_OBJ_MEMORY: u32 = 0x400;
/// DirectX 7's: the object is in the stream at pStream. Programs for
/// DirectX 6.1 leave it clear.
pub const DMUS_OBJ_STREAM: u32 = 0x800;

/// DMUS_OBJECTDESC's places and size.
const DESC_NAME: u32 = 56;
const DESC_CATEGORY: u32 = 184;
const DESC_FILE: u32 = 312;
pub const DESC_MEM_LENGTH: u32 = 832;
pub const DESC_MEM_DATA: u32 = 840;
/// pStream, DirectX 7's, in what was DirectX 6.1's padding.
pub const DESC_STREAM: u32 = 844;
pub const DESC_SIZE: u32 = 848;
/// The characters of the names (with their 0).
const NAME_CHARS: usize = 64;
const FILE_CHARS: usize = 260;

impl Desc {
    /// The description at `at`, as far as its flags say.
    pub fn read(cpu: &mut Cpu, at: u32) -> Desc {
        let mut d = Desc {
            valid: cpu.bus.guest_read_32(at + 4),
            ..Desc::default()
        };
        cpu.bus.guest_read_bytes(at + 8, &mut d.object);
        cpu.bus.guest_read_bytes(at + 24, &mut d.class);
        d.date = read_u64(cpu, at + 40);
        d.version = (
            cpu.bus.guest_read_32(at + 48),
            cpu.bus.guest_read_32(at + 52),
        );
        if d.valid & DMUS_OBJ_NAME != 0 {
            d.name = read_wide(cpu, at + DESC_NAME, NAME_CHARS);
        }
        if d.valid & DMUS_OBJ_CATEGORY != 0 {
            d.category = read_wide(cpu, at + DESC_CATEGORY, NAME_CHARS);
        }
        if d.valid & DMUS_OBJ_FILENAME != 0 {
            d.file = read_wide(cpu, at + DESC_FILE, FILE_CHARS);
        }
        d
    }

    /// Write it to the description at `at`: its flags and the fields
    /// they name, leaving dwSize.
    pub fn write(&self, cpu: &mut Cpu, at: u32) {
        cpu.bus.guest_write_32(at + 4, self.valid);
        let v = self.valid;
        if v & DMUS_OBJ_OBJECT != 0 {
            cpu.bus.guest_write_bytes(at + 8, &self.object);
        }
        if v & DMUS_OBJ_CLASS != 0 {
            cpu.bus.guest_write_bytes(at + 24, &self.class);
        }
        if v & DMUS_OBJ_DATE != 0 {
            write_u64(cpu, at + 40, self.date);
        }
        if v & DMUS_OBJ_VERSION != 0 {
            cpu.bus.guest_write_32(at + 48, self.version.0);
            cpu.bus.guest_write_32(at + 52, self.version.1);
        }
        if v & DMUS_OBJ_NAME != 0 {
            write_wide(cpu, at + DESC_NAME, &self.name, NAME_CHARS);
        }
        if v & DMUS_OBJ_CATEGORY != 0 {
            write_wide(cpu, at + DESC_CATEGORY, &self.category, NAME_CHARS);
        }
        if v & DMUS_OBJ_FILENAME != 0 {
            write_wide(cpu, at + DESC_FILE, &self.file, FILE_CHARS);
        }
    }

    /// Take what `other` has over what this has.
    pub fn merge(&mut self, other: &Desc) {
        let v = other.valid;
        if v & DMUS_OBJ_OBJECT != 0 {
            self.object = other.object;
        }
        if v & DMUS_OBJ_CLASS != 0 {
            self.class = other.class;
        }
        if v & DMUS_OBJ_NAME != 0 {
            self.name = other.name.clone();
        }
        if v & DMUS_OBJ_CATEGORY != 0 {
            self.category = other.category.clone();
        }
        if v & DMUS_OBJ_FILENAME != 0 {
            self.file = other.file.clone();
        }
        if v & DMUS_OBJ_VERSION != 0 {
            self.version = other.version;
        }
        if v & DMUS_OBJ_DATE != 0 {
            self.date = other.date;
        }
        self.valid |= v & !(DMUS_OBJ_MEMORY | DMUS_OBJ_STREAM | DMUS_OBJ_FULLPATH);
    }
}

/// An object of DirectMusic's.
#[derive(Clone, Debug, Default)]
pub struct Object {
    pub id: u32,
    pub pid: u32,
    pub refs: u32,
    /// Its interfaces' blocks so far: (`Iface::key`, address).
    pub ifaces: Vec<(u32, u32)>,
    pub desc: Desc,
    pub kind: Kind,
}

/// What an object is, and its state.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Kind {
    Music { m: core::Music },
    Port { p: core::Port },
    Buffer { b: core::Buf },
    Clock {},
    Collection { c: core::Collection },
    Instrument { patch: u32, name: String },
    Downloaded {},
    Loader { l: loader::Loader },
    Performance { p: performance::Perf },
    Segment { s: segment::Segment },
    SegmentState { s: performance::Playing },
    Graph { tools: Vec<u32> },
    Track { t: segment::Track },
    Band { b: content::Band },
    Style { s: content::Style },
    ChordMap { scale: u32 },
    Composer {},
}

impl Default for Kind {
    fn default() -> Kind {
        Kind::Clock {}
    }
}

impl Kind {
    /// Its interfaces: what QueryInterface gives for each IID. The first
    /// is the one IUnknown is.
    fn interfaces(&self) -> &'static [(Guid, Iface)] {
        match self {
            Kind::Music { .. } => &[(IID_IDIRECTMUSIC, MUSIC), (IID_IDIRECTMUSIC2, MUSIC)],
            Kind::Port { .. } => &[(IID_IDIRECTMUSIC_PORT, PORT)],
            Kind::Buffer { .. } => &[(IID_IDIRECTMUSIC_BUFFER, BUFFER)],
            Kind::Clock {} => &[(IID_IREFERENCE_CLOCK, CLOCK)],
            Kind::Collection { .. } => &[
                (IID_IDIRECTMUSIC_COLLECTION, COLLECTION),
                (IID_IDIRECTMUSIC_OBJECT, DMUSIC_OBJECT),
                (IID_IPERSIST_STREAM, DMUSIC_PERSIST),
                (IID_IPERSIST, DMUSIC_PERSIST),
            ],
            Kind::Instrument { .. } => &[(IID_IDIRECTMUSIC_INSTRUMENT, INSTRUMENT)],
            Kind::Downloaded {} => &[(IID_IDIRECTMUSIC_DOWNLOADED_INSTRUMENT, DOWNLOADED)],
            Kind::Loader { .. } => &[(IID_IDIRECTMUSIC_LOADER, LOADER)],
            Kind::Performance { .. } => &[
                (IID_IDIRECTMUSIC_PERFORMANCE, PERFORMANCE),
                (IID_IDIRECTMUSIC_GRAPH, GRAPH),
            ],
            Kind::Segment { .. } => &[
                (IID_IDIRECTMUSIC_SEGMENT, SEGMENT),
                (IID_IDIRECTMUSIC_OBJECT, DMIME_OBJECT),
                (IID_IPERSIST_STREAM, DMIME_PERSIST),
                (IID_IPERSIST, DMIME_PERSIST),
            ],
            Kind::SegmentState { .. } => &[(IID_IDIRECTMUSIC_SEGMENT_STATE, SEGMENT_STATE)],
            Kind::Graph { .. } => &[
                (IID_IDIRECTMUSIC_GRAPH, GRAPH),
                (IID_IPERSIST_STREAM, DMIME_PERSIST),
            ],
            Kind::Track { .. } => &[
                (IID_IDIRECTMUSIC_TRACK, TRACK),
                (IID_IPERSIST_STREAM, DMIME_PERSIST),
                (IID_IPERSIST, DMIME_PERSIST),
            ],
            Kind::Band { .. } => &[
                (IID_IDIRECTMUSIC_BAND, BAND),
                (IID_IDIRECTMUSIC_OBJECT, DMBAND_OBJECT),
                (IID_IPERSIST_STREAM, DMBAND_PERSIST),
                (IID_IPERSIST, DMBAND_PERSIST),
            ],
            Kind::Style { .. } => &[
                (IID_IDIRECTMUSIC_STYLE, STYLE),
                (IID_IDIRECTMUSIC_OBJECT, DMSTYLE_OBJECT),
                (IID_IPERSIST_STREAM, DMSTYLE_PERSIST),
            ],
            Kind::ChordMap { .. } => &[
                (IID_IDIRECTMUSIC_CHORD_MAP, CHORD_MAP),
                (IID_IDIRECTMUSIC_OBJECT, DMCOMPOS_OBJECT),
                (IID_IPERSIST_STREAM, DMCOMPOS_PERSIST),
            ],
            Kind::Composer {} => &[(IID_IDIRECTMUSIC_COMPOSER, COMPOSER)],
        }
    }

    /// The objects it holds references on.
    fn held(&self) -> Vec<u32> {
        match self {
            Kind::Music { m } => m.held(),
            Kind::Loader { l } => l.held(),
            Kind::Performance { p } => p.held(),
            Kind::Segment { s } => s.held(),
            Kind::SegmentState { s } => vec![s.segment],
            Kind::Track { t } => t.held(),
            Kind::Style { s } => s.bands.iter().map(|b| b.1).collect(),
            _ => Vec::new(),
        }
    }
}

/// DirectMusic's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub objects: Vec<Object>,
    pub last_id: u32,
}

pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.dmusic
}

impl State {
    pub fn get(&self, id: u32) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }
}

/// Make an object of the running process, with one reference.
pub fn add(cpu: &mut Cpu, kind: Kind) -> u32 {
    let pid = super::user32::current_pid(cpu);
    let s = state(cpu);
    s.last_id += 1;
    let id = s.last_id;
    s.objects.push(Object {
        id,
        pid,
        refs: 1,
        ifaces: Vec::new(),
        desc: Desc::default(),
        kind,
    });
    id
}

/// Take a reference on the object `id`.
pub fn hold(cpu: &mut Cpu, id: u32) {
    if let Some(o) = state(cpu).get_mut(id) {
        o.refs += 1;
    }
}

/// Give back a reference on the object `id`: its count left. With none
/// left it goes, and what it held is released.
pub fn drop_ref(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(o) = state(cpu).get_mut(id) else {
        return 0;
    };
    o.refs = o.refs.saturating_sub(1);
    if o.refs > 0 {
        return o.refs;
    }
    if matches!(o.kind, Kind::Performance { .. }) {
        performance::close(cpu, id);
    }
    let s = state(cpu);
    let Some(i) = s.objects.iter().position(|o| o.id == id) else {
        return 0;
    };
    let gone = s.objects.remove(i);
    for &(_, at) in &gone.ifaces {
        com::free_interface(cpu, at);
    }
    for held in gone.kind.held() {
        drop_ref(cpu, held);
    }
    0
}

/// The object `this` is an interface of: its id and the interface.
pub fn interface(cpu: &mut Cpu, this: u32) -> Option<(u32, Iface)> {
    if this == 0 || !cpu.bus.guest_probe(this, com::HEADER as usize, false) {
        return None;
    }
    for (slot, dll) in DLLS.iter().enumerate() {
        let index = super::stubs::find_dll(dll.name)?;
        if let Some((vtable, id)) = com::interface_of(cpu, index, this) {
            return Some((id, Iface(slot as u8, vtable as u8)));
        }
    }
    None
}

/// The object `this` is an interface of, if it is the interface `want`.
pub fn this_of(cpu: &mut Cpu, this: u32, want: Iface) -> Option<u32> {
    interface(cpu, this)
        .filter(|&(_, i)| i == want)
        .map(|(id, _)| id)
        .filter(|&id| state(cpu).get(id).is_some())
}

/// The object an interface pointer the program passed is, whichever of
/// its interfaces it is.
pub fn object_of(cpu: &mut Cpu, this: u32) -> Option<u32> {
    interface(cpu, this)
        .map(|(id, _)| id)
        .filter(|&id| state(cpu).get(id).is_some())
}

/// The block of the interface `iface` of the object `id`, made the first
/// time it is asked for. None if the heap is full.
pub fn block(cpu: &mut Cpu, id: u32, iface: Iface) -> Option<u32> {
    let o = state(cpu).get(id)?;
    if let Some(&(_, at)) = o.ifaces.iter().find(|(k, _)| *k == iface.key()) {
        return Some(at);
    }
    let name = DLLS[iface.0 as usize].name;
    let dll = super::stubs::find_dll(name)?;
    // The methods are in the DLL's image, which a program that made the
    // object through another of the DLLs hasn't mapped yet.
    super::loader::map_builtin_named(cpu, name)?;
    let at = com::new_interface(cpu, dll, iface.1 as usize, id, 0)?;
    state(cpu).get_mut(id)?.ifaces.push((iface.key(), at));
    Some(at)
}

/// The object `id`'s interface `iid` to `out`, with a reference taken:
/// S_OK, or E_NOINTERFACE (with NULL at `out`).
pub fn give(cpu: &mut Cpu, id: u32, iid: &Guid, out: u32) -> u32 {
    let Some(o) = state(cpu).get(id) else {
        com::write_out(cpu, out, 0);
        return E_NOINTERFACE;
    };
    let list = o.kind.interfaces();
    let found = if *iid == IID_IUNKNOWN {
        list.first()
    } else {
        list.iter().find(|(g, _)| g == iid)
    };
    let Some(&(_, iface)) = found else {
        com::write_out(cpu, out, 0);
        return E_NOINTERFACE;
    };
    let Some(at) = block(cpu, id, iface) else {
        com::write_out(cpu, out, 0);
        return E_OUTOFMEMORY;
    };
    hold(cpu, id);
    com::write_out(cpu, out, at);
    S_OK
}

/// A new object's interface `iid` to `out` for a class's `create`: the
/// object is released if it hasn't the interface.
pub fn give_new(cpu: &mut Cpu, id: u32, iid: &Guid, out: u32) -> u32 {
    let hr = give(cpu, id, iid, out);
    drop_ref(cpu, id);
    hr
}

/// An interface of the object `id` to give the program a pointer to,
/// with a reference taken: the first one it has (its own).
pub fn pointer(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(&(_, iface)) = state(cpu).get(id).and_then(|o| o.kind.interfaces().first()) else {
        return 0;
    };
    let Some(at) = block(cpu, id, iface) else {
        return 0;
    };
    hold(cpu, id);
    at
}

/// A process ended: its objects go, and a performance of it that played
/// ends its notes.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let s = state(cpu);
    let played = s.objects.iter().any(|o| {
        o.pid == pid && matches!(&o.kind, Kind::Performance { p } if !p.states.is_empty())
    });
    let events: Vec<u32> = s
        .objects
        .iter()
        .filter(|o| o.pid == pid)
        .filter_map(|o| match &o.kind {
            Kind::Performance { p } => (p.event != 0).then_some(p.event),
            _ => None,
        })
        .collect();
    s.objects.retain(|o| o.pid != pid);
    for e in events {
        super::object::objects(cpu).release(e);
    }
    if played {
        super::winmm::midi::notes_off(cpu);
    }
}

/// The process `pid`'s performances send what is due at `now`
/// (microseconds), on its multimedia thread (winmm/callback.rs).
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    performance::poll(cpu, pid, now);
}

/// When the process `pid`'s performances next have something due.
pub fn next_due(cpu: &Cpu, pid: Option<u32>) -> Option<u64> {
    performance::next_due(&cpu.bus.win32.dmusic, pid)
}

/// The scale the performances' master volume puts on the synthesizer.
pub fn gain(s: &State) -> f32 {
    performance::gain(s)
}

/// DllCanUnloadNow: S_FALSE while the process has objects.
fn dll_can_unload_now(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let pid = super::user32::current_pid(cpu);
    let used = state(cpu).objects.iter().any(|o| o.pid == pid);
    Some(if used { S_FALSE } else { S_OK })
}

fn dll_get_class_object_dmusic(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DMUSIC.name, args))
}

fn dll_get_class_object_dmime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DMIME.name, args))
}

fn dll_get_class_object_dmloader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DMLOADER.name, args))
}

fn dll_get_class_object_dmstyle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DMSTYLE.name, args))
}

fn dll_get_class_object_dmband(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DMBAND.name, args))
}

fn dll_get_class_object_dmcompos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DMCOMPOS.name, args))
}

fn s_ok(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

fn s_false(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_FALSE)
}

fn e_notimpl(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

/// IUnknown::QueryInterface(this, iid, out), for every interface here.
fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let Some(id) = object_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_POINTER);
    };
    Some(give(cpu, id, &iid, out))
}

/// IUnknown::AddRef(this): the object's count.
fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    hold(cpu, id);
    Some(state(cpu).get(id).map_or(0, |o| o.refs))
}

/// IUnknown::Release(this): the object's count left.
fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    Some(drop_ref(cpu, id))
}

/// The class of an object, for IPersist::GetClassID and its description.
fn class_of(kind: &Kind) -> Guid {
    match kind {
        Kind::Collection { .. } => CLSID_COLLECTION,
        Kind::Segment { .. } => CLSID_SEGMENT,
        Kind::Graph { .. } => CLSID_GRAPH,
        Kind::Track { t } => Guid(t.class),
        Kind::Band { .. } => CLSID_BAND,
        Kind::Style { .. } => CLSID_STYLE,
        Kind::ChordMap { .. } => CLSID_CHORD_MAP,
        _ => com::GUID_NULL,
    }
}

/// IDirectMusicObject::GetDescriptor(this, desc).
fn get_descriptor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, DESC_MEM_LENGTH as usize, true) {
        return Some(E_POINTER);
    }
    let o = state(cpu).get(id)?;
    let mut d = o.desc.clone();
    d.class = class_of(&o.kind).0;
    d.valid |= DMUS_OBJ_CLASS;
    // The description is filled from the start, its size kept.
    let size = cpu.bus.guest_read_32(at);
    cpu.bus.guest_fill(at + 4, DESC_MEM_LENGTH as usize - 4, 0);
    d.write(cpu, at);
    cpu.bus.guest_write_32(at, size);
    Some(S_OK)
}

/// IDirectMusicObject::SetDescriptor(this, desc).
fn set_descriptor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let d = Desc::read(cpu, args[1]);
    if let Some(o) = state(cpu).get_mut(id) {
        o.desc.merge(&d);
    }
    Some(S_OK)
}

/// The file at the position of the stream `stream`, which it reads past:
/// a RIFF file's chunk, or a MIDI file's rest of the stream. Streams on
/// global memory (OLE32's CreateStreamOnHGlobal) are read; a program's
/// own IStream isn't, and gives E_NOTIMPL.
pub fn stream_data(cpu: &mut Cpu, stream: u32) -> Result<Vec<u8>, u32> {
    let Some(mut data) = super::ole32::stream::rest_of(cpu, stream) else {
        return Err(E_NOTIMPL);
    };
    if data.starts_with(b"RIFF") && data.len() >= 8 {
        let size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        data.truncate(size.saturating_add(8));
    }
    super::ole32::stream::advance(cpu, stream, data.len() as u32);
    Ok(data)
}

/// IDirectMusicObject::ParseDescriptor(this, stream, desc): the
/// description the file in the stream keeps, with its class;
/// DMUS_E_INVALIDFILE when the stream holds no file of the object's
/// class.
fn parse_descriptor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stream, at] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if stream == 0 || at == 0 {
        return Some(E_POINTER);
    }
    let Some(id) = object_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if !cpu.bus.guest_probe(at, DESC_MEM_LENGTH as usize, true) {
        return Some(E_POINTER);
    }
    let class = class_of(&state(cpu).get(id)?.kind);
    let data = match stream_data(cpu, stream) {
        Ok(data) => data,
        Err(e) => return Some(e),
    };
    if files::class_of(&data) != Some(class.0) {
        return Some(DMUS_E_INVALIDFILE);
    }
    let Some((mut d, _)) = files::read(&data) else {
        return Some(DMUS_E_INVALIDFILE);
    };
    d.class = class.0;
    d.valid |= DMUS_OBJ_CLASS;
    let size = cpu.bus.guest_read_32(at);
    cpu.bus.guest_fill(at + 4, DESC_MEM_LENGTH as usize - 4, 0);
    d.write(cpu, at);
    cpu.bus.guest_write_32(at, size);
    Some(S_OK)
}

/// IPersistStream::Load(this, stream): the object becomes what the file
/// in the stream holds, which must be of its class.
fn persist_load(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stream] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if stream == 0 {
        return Some(E_POINTER);
    }
    let Some(id) = object_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let class = class_of(&state(cpu).get(id)?.kind);
    let data = match stream_data(cpu, stream) {
        Ok(data) => data,
        Err(e) => return Some(e),
    };
    if files::class_of(&data) != Some(class.0) {
        return Some(DMUS_E_UNSUPPORTED_STREAM);
    }
    let Some((desc, content)) = files::read(&data) else {
        return Some(DMUS_E_UNSUPPORTED_STREAM);
    };
    // The content is made as a new object, whose state goes into this
    // one; the new object takes the old state with it as it goes.
    let made = loader::make(cpu, content);
    let s = state(cpu);
    let mut kind = std::mem::take(&mut s.get_mut(made)?.kind);
    std::mem::swap(&mut s.get_mut(id)?.kind, &mut kind);
    s.get_mut(made)?.kind = kind;
    s.get_mut(id)?.desc.merge(&desc);
    drop_ref(cpu, made);
    Some(S_OK)
}

/// IPersist::GetClassID(this, clsid).
fn get_class_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let class = class_of(&state(cpu).get(id)?.kind);
    Some(if class.write(cpu, args[1]) {
        S_OK
    } else {
        E_POINTER
    })
}

/// Memory of the running process's heap, zeroed: its address, or None.
pub fn alloc(cpu: &mut Cpu, size: u32) -> Option<u32> {
    let at = super::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size))?;
    cpu.bus.guest_fill(at, size as usize, 0);
    Some(at)
}

pub fn free(cpu: &mut Cpu, at: u32) {
    super::with_process(cpu, |p, _| p.heap.free(at));
}

pub fn read_u64(cpu: &mut Cpu, at: u32) -> u64 {
    cpu.bus.guest_read_32(at) as u64 | (cpu.bus.guest_read_32(at + 4) as u64) << 32
}

pub fn write_u64(cpu: &mut Cpu, at: u32, value: u64) {
    cpu.bus.guest_write_32(at, value as u32);
    cpu.bus.guest_write_32(at + 4, (value >> 32) as u32);
}

/// A 64-bit argument from its two words.
pub fn arg64(lo: u32, hi: u32) -> i64 {
    (lo as u64 | (hi as u64) << 32) as i64
}

/// The string of 16-bit characters at `at`, of at most `max` characters.
pub fn read_wide(cpu: &mut Cpu, at: u32, max: usize) -> String {
    let mut units = Vec::new();
    for i in 0..max as u32 {
        let u = cpu.bus.guest_read_16(at + 2 * i);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16_lossy(&units)
}

/// Write `s` as 16-bit characters with a 0 at `at`, cut to `max`
/// characters with the 0.
pub fn write_wide(cpu: &mut Cpu, at: u32, s: &str, max: usize) {
    if max == 0 {
        return;
    }
    let mut units: Vec<u16> = s.encode_utf16().take(max - 1).collect();
    units.push(0);
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(at, &bytes);
}

crate::state_fields!(Desc {
    valid,
    object,
    class,
    name,
    category,
    file,
    version,
    date
});
crate::state_fields!(Object {
    id,
    pid,
    refs,
    ifaces,
    desc,
    kind
});
super::state_variants!(Kind {
    Music { m },
    Port { p },
    Buffer { b },
    Clock {},
    Collection { c },
    Instrument { patch, name },
    Downloaded {},
    Loader { l },
    Performance { p },
    Segment { s },
    SegmentState { s },
    Graph { tools },
    Track { t },
    Band { b },
    Style { s },
    ChordMap { scale },
    Composer {},
});
crate::state_fields!(State { objects, last_id });
