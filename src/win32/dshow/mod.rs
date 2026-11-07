//! DirectShow, as Windows 98 SE has it (DirectX Media 6): QUARTZ.DLL,
//! the filter graph manager (graph.rs), and AMSTREAM.DLL, the multimedia
//! streams that read a movie's pictures into DirectDraw surfaces and its
//! sound into buffers (stream.rs). Programs make their objects with
//! CoCreateInstance (ole32.rs) or the DLLs' DllGetClassObject.
//!
//! Both play the files the digital video devices of MCI play (winmm/mci
//! media.rs): AVI files of uncompressed pictures or the Video for
//! Windows codecs (Indeo 3, 4 and 5), with PCM sound, and MPEG-1. The
//! filter graph plays its file through an MPEGVideo device of its own,
//! in the video renderer's window, "ActiveMovie Window"; the multimedia
//! stream decodes each picture into the sample's surface when the
//! program asks for it, at the picture's time.
//!
//! Built-in classes come before a registry's InprocServer32
//! (ole32/compobj.rs), so a program gets these objects even where an
//! installer registered a QUARTZ.DLL of its own.
//!
//! Each object is in `State::objects`, with one reference count for all
//! its interfaces, as DirectShow's objects have. A multimedia stream
//! holds references on its media streams, and a sample on its stream.

/// The value of a `Result` whose error is an HRESULT, or the method
/// returns the error.
macro_rules! hr {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Some(e),
        }
    };
}

pub mod filter;
pub mod graph;
pub mod sample;
pub mod stream;

use super::com::{self, Guid};
use super::stubs::{Api, Conv, Dll, Handler, Vtable};
use crate::cpu::Cpu;

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

pub static QUARTZ: Dll = Dll {
    name: "QUARTZ.DLL",
    base: 0xBF62_0000,
    apis: QUARTZ_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: QUARTZ_VTABLES,
};

pub static AMSTREAM: Dll = Dll {
    name: "AMSTREAM.DLL",
    base: 0xBF64_0000,
    apis: AMSTREAM_APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: AMSTREAM_VTABLES,
};

/// The DLLs, by their places in `Iface::0`.
static DLLS: [&Dll; 2] = [&QUARTZ, &AMSTREAM];
const D_QUARTZ: u8 = 0;
const D_AMSTREAM: u8 = 1;

// The GUIDs.

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    Guid::new(d1, d2, d3, d4)
}

/// The filter graph's, {xxxxxxxx-0AD4-11CE-B03A-0020AF0BA770}.
const fn quartz_iid(d1: u32) -> Guid {
    guid(
        d1,
        0x0AD4,
        0x11CE,
        [0xB0, 0x3A, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
    )
}

pub const CLSID_FILTER_GRAPH: Guid = guid(
    0xE436_EBB3,
    0x524F,
    0x11CE,
    [0x9F, 0x53, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
);
pub const CLSID_FILTER_GRAPH_NO_THREAD: Guid = guid(
    0xE436_EBB8,
    0x524F,
    0x11CE,
    [0x9F, 0x53, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
);
pub const IID_IFILTER_GRAPH: Guid = quartz_iid(0x56A8_689F);
pub const IID_IGRAPH_BUILDER: Guid = quartz_iid(0x56A8_68A9);
pub const IID_IMEDIA_CONTROL: Guid = quartz_iid(0x56A8_68B1);
pub const IID_IMEDIA_POSITION: Guid = quartz_iid(0x56A8_68B2);
pub const IID_IBASIC_AUDIO: Guid = quartz_iid(0x56A8_68B3);
pub const IID_IVIDEO_WINDOW: Guid = quartz_iid(0x56A8_68B4);
pub const IID_IBASIC_VIDEO: Guid = quartz_iid(0x56A8_68B5);
pub const IID_IMEDIA_EVENT: Guid = quartz_iid(0x56A8_68B6);
pub const IID_IMEDIA_EVENT_EX: Guid = quartz_iid(0x56A8_68C0);
pub const IID_IMEDIA_SEEKING: Guid = guid(
    0x36B7_3880,
    0xC2C8,
    0x11CF,
    [0x8B, 0x46, 0x00, 0x80, 0x5F, 0x6C, 0xEF, 0x60],
);
pub const IID_IMEDIA_EVENT_SINK: Guid = quartz_iid(0x56A8_68A2);
pub const IID_IBASE_FILTER: Guid = quartz_iid(0x56A8_6895);
pub const IID_IMEDIA_FILTER: Guid = quartz_iid(0x56A8_6899);
pub const IID_IPERSIST: Guid = guid(0x0000_010C, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IPIN: Guid = quartz_iid(0x56A8_6891);
pub const IID_IMEM_INPUT_PIN: Guid = quartz_iid(0x56A8_689D);
pub const IID_IMEM_ALLOCATOR: Guid = quartz_iid(0x56A8_689C);
pub const IID_IMEDIA_SAMPLE: Guid = quartz_iid(0x56A8_689A);
pub const IID_IMEDIA_SAMPLE2: Guid = guid(
    0x36B7_3884,
    0xC2C8,
    0x11CF,
    [0x8B, 0x46, 0x00, 0x80, 0x5F, 0x6C, 0xEF, 0x60],
);
pub const IID_IREFERENCE_CLOCK: Guid = quartz_iid(0x56A8_6897);
pub const IID_IQUALITY_CONTROL: Guid = quartz_iid(0x56A8_68A5);
pub const CLSID_MEMORY_ALLOCATOR: Guid = guid(
    0x1E65_1CC0,
    0xB199,
    0x11D0,
    [0x82, 0x12, 0x00, 0xC0, 0x4F, 0xC3, 0x2C, 0x45],
);
pub const CLSID_SYSTEM_CLOCK: Guid = guid(
    0xE436_EBB1,
    0x524F,
    0x11CE,
    [0x9F, 0x53, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
);
pub const CLSID_MPEG_VIDEO_CODEC: Guid = guid(
    0xFEB5_0740,
    0x7BEF,
    0x11CE,
    [0x9B, 0xD9, 0x00, 0x00, 0xE2, 0x02, 0x59, 0x9C],
);
pub const CLSID_MPEG_AUDIO_CODEC: Guid = guid(
    0x4A22_86E0,
    0x7BEF,
    0x11CE,
    [0x9B, 0xD9, 0x00, 0x00, 0xE2, 0x02, 0x59, 0x9C],
);
pub const CLSID_AVI_SPLITTER: Guid = guid(
    0x1B54_4C20,
    0xFD0B,
    0x11CE,
    [0x8C, 0x63, 0x00, 0xAA, 0x00, 0x44, 0xB5, 0x1E],
);
pub const TIME_FORMAT_MEDIA_TIME: Guid = guid(
    0x7B78_5574,
    0x8C82,
    0x11CF,
    [0xBC, 0x0C, 0x00, 0xAA, 0x00, 0xAC, 0x74, 0xF6],
);

/// amstream's, {xxxxxxxx-yyyy-11D0-8FDE-00C04FD9189D} and
/// {xxxxxxxx-yyyy-11D0-8212-00C04FC32C45}.
const fn ms_iid(d1: u32, d2: u16) -> Guid {
    guid(
        d1,
        d2,
        0x11D0,
        [0x8F, 0xDE, 0x00, 0xC0, 0x4F, 0xD9, 0x18, 0x9D],
    )
}
const fn ms_iid2(d1: u32, d2: u16) -> Guid {
    guid(
        d1,
        d2,
        0x11D0,
        [0x82, 0x12, 0x00, 0xC0, 0x4F, 0xC3, 0x2C, 0x45],
    )
}

pub const CLSID_AM_MULTI_MEDIA_STREAM: Guid = ms_iid2(0x49C4_7CE5, 0x9BA4);
pub const CLSID_AM_AUDIO_DATA: Guid = ms_iid2(0xF246_8580, 0xAF8A);
pub const IID_IMULTI_MEDIA_STREAM: Guid = ms_iid(0xB502_D1BC, 0x9A57);
pub const IID_IMEDIA_STREAM: Guid = ms_iid(0xB502_D1BD, 0x9A57);
pub const IID_ISTREAM_SAMPLE: Guid = ms_iid(0xB502_D1BE, 0x9A57);
pub const IID_IAM_MULTI_MEDIA_STREAM: Guid = ms_iid(0xBEBE_595C, 0x9A6F);
pub const IID_IDIRECT_DRAW_MEDIA_STREAM: Guid = ms_iid(0xF410_4FCE, 0x9A70);
pub const IID_IDIRECT_DRAW_STREAM_SAMPLE: Guid = ms_iid(0xF410_4FCF, 0x9A70);
pub const IID_IAUDIO_MEDIA_STREAM: Guid = ms_iid2(0xF753_7560, 0xA3BE);
pub const IID_IAUDIO_STREAM_SAMPLE: Guid = ms_iid2(0x345F_EE00, 0xABA5);
pub const IID_IMEMORY_DATA: Guid = ms_iid2(0x327F_C560, 0xAF60);
pub const IID_IAUDIO_DATA: Guid = ms_iid2(0x54C7_19C0, 0xAF60);
pub const MSPID_PRIMARY_VIDEO: Guid = guid(
    0xA35F_F56A,
    0x9FDA,
    0x11D0,
    [0x8F, 0xDF, 0x00, 0xC0, 0x4F, 0xD9, 0x18, 0x9D],
);
pub const MSPID_PRIMARY_AUDIO: Guid = guid(
    0xA35F_F56B,
    0x9FDA,
    0x11D0,
    [0x8F, 0xDF, 0x00, 0xC0, 0x4F, 0xD9, 0x18, 0x9D],
);

// The HRESULTs.

pub const S_OK: u32 = com::S_OK;
pub const S_FALSE: u32 = com::S_FALSE;
pub const E_NOTIMPL: u32 = com::E_NOTIMPL;
pub const E_NOINTERFACE: u32 = com::E_NOINTERFACE;
pub const E_POINTER: u32 = com::E_POINTER;
pub const E_INVALIDARG: u32 = com::E_INVALIDARG;
pub const E_OUTOFMEMORY: u32 = com::E_OUTOFMEMORY;
pub const E_ABORT: u32 = 0x8000_4004;
pub const E_FAIL: u32 = 0x8000_4005;
pub const E_UNEXPECTED: u32 = 0x8000_FFFF;
/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND).
pub const FILE_NOT_FOUND: u32 = 0x8007_0002;
pub const VFW_E_UNKNOWN_FILE_TYPE: u32 = 0x8004_0240;
pub const VFW_E_NO_PALETTE_AVAILABLE: u32 = 0x8004_0220;
pub const VFW_E_WRONG_STATE: u32 = 0x8004_0227;
pub const VFW_S_DUPLICATE_NAME: u32 = 0x0004_022D;
pub const VFW_S_NO_STOP_TIME: u32 = 0x0004_0270;
pub const VFW_E_NOT_CONNECTED: u32 = 0x8004_0209;
pub const VFW_E_ALREADY_CONNECTED: u32 = 0x8004_0204;
pub const VFW_E_BUFFER_NOTSET: u32 = 0x8004_020C;
pub const VFW_E_BUFFER_OVERFLOW: u32 = 0x8004_020D;
pub const VFW_E_BADALIGN: u32 = 0x8004_020E;
pub const VFW_E_ALREADY_COMMITTED: u32 = 0x8004_020F;
pub const VFW_E_BUFFERS_OUTSTANDING: u32 = 0x8004_0210;
pub const VFW_E_NOT_COMMITTED: u32 = 0x8004_0211;
pub const VFW_E_SIZENOTSET: u32 = 0x8004_0212;
pub const VFW_E_NOT_FOUND: u32 = 0x8004_0216;
pub const VFW_E_NOT_STOPPED: u32 = 0x8004_0224;
pub const VFW_E_TIMEOUT: u32 = 0x8004_022E;
pub const VFW_E_SAMPLE_TIME_NOT_SET: u32 = 0x8004_0249;
pub const VFW_E_MEDIA_TIME_NOT_SET: u32 = 0x8004_0251;

// The tables.

/// IUnknown's methods.
const UNKNOWN: [&str; 3] = ["QueryInterface", "AddRef", "Release"];

/// The methods of an interface that derives from IDispatch: IUnknown's,
/// IDispatch's and `own`.
macro_rules! dispatch {
    ($($m:literal),* $(,)?) => {
        &[
            "QueryInterface",
            "AddRef",
            "Release",
            "Dispatch_GetTypeInfoCount",
            "Dispatch_GetTypeInfo",
            "Dispatch_GetIDsOfNames",
            "Dispatch_Invoke",
            $($m),*
        ]
    };
}

/// The methods of an interface that derives from IUnknown alone.
macro_rules! unknown {
    ($($m:literal),* $(,)?) => {
        &["QueryInterface", "AddRef", "Release", $($m),*]
    };
}

/// QUARTZ.DLL's functions. Append only (see `stubs`).
static QUARTZ_APIS: &[Api] = &[
    stdcall("AMGetErrorTextA", 1, 3, graph::am_get_error_text_a),
    stdcall("AMGetErrorTextW", 2, 3, graph::am_get_error_text_w),
    stdcall("AmpFactorToDB", 3, 1, graph::amp_factor_to_db),
    stdcall("DBToAmpFactor", 4, 1, graph::db_to_amp_factor),
    stdcall("DllCanUnloadNow", 5, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 6, 3, dll_get_class_object_quartz),
    stdcall("DllRegisterServer", 7, 0, s_ok),
    stdcall("DllUnregisterServer", 8, 0, s_ok),
    method(UNKNOWN[0], 3, query_interface),
    method(UNKNOWN[1], 1, add_ref),
    method(UNKNOWN[2], 1, release),
    method("Dispatch_GetTypeInfoCount", 2, graph::get_type_info_count),
    method("Dispatch_GetTypeInfo", 4, e_notimpl),
    method("Dispatch_GetIDsOfNames", 6, e_notimpl),
    method("Dispatch_Invoke", 9, e_notimpl),
    // IGraphBuilder (IFilterGraph's first).
    method("Graph_AddFilter", 3, filter::add_filter),
    method("Graph_RemoveFilter", 2, filter::remove_filter),
    method("Graph_EnumFilters", 2, e_notimpl),
    method("Graph_FindFilterByName", 3, filter::find_filter_by_name),
    method("Graph_ConnectDirect", 4, e_notimpl),
    method("Graph_Reconnect", 2, e_notimpl),
    method("Graph_Disconnect", 2, e_notimpl),
    method("Graph_SetDefaultSyncSource", 1, s_ok),
    method("Graph_Connect", 3, e_notimpl),
    method("Graph_Render", 2, e_notimpl),
    method("Graph_RenderFile", 3, graph::render_file),
    method("Graph_AddSourceFilter", 4, e_notimpl),
    method("Graph_SetLogFile", 2, s_ok),
    method("Graph_Abort", 1, s_ok),
    method("Graph_ShouldOperationContinue", 1, s_ok),
    // IMediaControl.
    method("Control_Run", 1, graph::run),
    method("Control_Pause", 1, graph::pause),
    method("Control_Stop", 1, graph::stop),
    method("Control_GetState", 3, graph::get_state),
    method("Control_RenderFile", 2, graph::control_render_file),
    method("Control_AddSourceFilter", 3, e_notimpl),
    method("Control_get_FilterCollection", 2, e_notimpl),
    method("Control_get_RegFilterCollection", 2, e_notimpl),
    method("Control_StopWhenReady", 1, graph::stop_when_ready),
    // IMediaEventEx (IMediaEvent's first).
    method("Event_GetEventHandle", 2, graph::get_event_handle),
    method("Event_GetEvent", 5, graph::get_event),
    method("Event_WaitForCompletion", 3, graph::wait_for_completion),
    method(
        "Event_CancelDefaultHandling",
        2,
        graph::cancel_default_handling,
    ),
    method(
        "Event_RestoreDefaultHandling",
        2,
        graph::restore_default_handling,
    ),
    method("Event_FreeEventParams", 4, s_ok),
    method("Event_SetNotifyWindow", 4, graph::set_notify_window),
    method("Event_SetNotifyFlags", 2, graph::set_notify_flags),
    method("Event_GetNotifyFlags", 2, graph::get_notify_flags),
    // IMediaPosition.
    method("Position_get_Duration", 2, graph::get_duration_seconds),
    method(
        "Position_put_CurrentPosition",
        3,
        graph::put_current_seconds,
    ),
    method(
        "Position_get_CurrentPosition",
        2,
        graph::get_current_seconds,
    ),
    method("Position_get_StopTime", 2, graph::get_stop_seconds),
    method("Position_put_StopTime", 3, graph::put_stop_seconds),
    method("Position_get_PrerollTime", 2, graph::get_zero_seconds),
    method("Position_put_PrerollTime", 3, s_ok),
    method("Position_put_Rate", 3, graph::put_rate),
    method("Position_get_Rate", 2, graph::get_rate),
    method("Position_CanSeekForward", 2, graph::can_seek),
    method("Position_CanSeekBackward", 2, graph::can_seek),
    // IMediaSeeking.
    method("Seeking_GetCapabilities", 2, graph::get_capabilities),
    method("Seeking_CheckCapabilities", 2, graph::check_capabilities),
    method("Seeking_IsFormatSupported", 2, graph::is_format_supported),
    method("Seeking_QueryPreferredFormat", 2, graph::get_time_format),
    method("Seeking_GetTimeFormat", 2, graph::get_time_format),
    method("Seeking_IsUsingTimeFormat", 2, graph::is_format_supported),
    method("Seeking_SetTimeFormat", 2, graph::set_time_format),
    method("Seeking_GetDuration", 2, graph::get_duration),
    method("Seeking_GetStopPosition", 2, graph::get_stop_position),
    method("Seeking_GetCurrentPosition", 2, graph::get_current_position),
    method("Seeking_ConvertTimeFormat", 6, graph::convert_time_format),
    method("Seeking_SetPositions", 5, graph::set_positions),
    method("Seeking_GetPositions", 3, graph::get_positions),
    method("Seeking_GetAvailable", 3, graph::get_available),
    method("Seeking_SetRate", 3, graph::put_rate),
    method("Seeking_GetRate", 2, graph::get_rate),
    method("Seeking_GetPreroll", 2, graph::get_preroll),
    // IVideoWindow.
    method("Window_put_Caption", 2, graph::put_caption),
    method("Window_get_Caption", 2, graph::get_caption),
    method("Window_put_WindowStyle", 2, graph::put_window_style),
    method("Window_get_WindowStyle", 2, graph::get_window_style),
    method("Window_put_WindowStyleEx", 2, graph::put_window_style_ex),
    method("Window_get_WindowStyleEx", 2, graph::get_window_style_ex),
    method("Window_put_AutoShow", 2, graph::put_auto_show),
    method("Window_get_AutoShow", 2, graph::get_auto_show),
    method("Window_put_WindowState", 2, graph::put_window_state),
    method("Window_get_WindowState", 2, graph::get_window_state),
    method(
        "Window_put_BackgroundPalette",
        2,
        graph::put_background_palette,
    ),
    method(
        "Window_get_BackgroundPalette",
        2,
        graph::get_background_palette,
    ),
    method("Window_put_Visible", 2, graph::put_visible),
    method("Window_get_Visible", 2, graph::get_visible),
    method("Window_put_Left", 2, graph::put_left),
    method("Window_get_Left", 2, graph::get_left),
    method("Window_put_Width", 2, graph::put_width),
    method("Window_get_Width", 2, graph::get_width),
    method("Window_put_Top", 2, graph::put_top),
    method("Window_get_Top", 2, graph::get_top),
    method("Window_put_Height", 2, graph::put_height),
    method("Window_get_Height", 2, graph::get_height),
    method("Window_put_Owner", 2, graph::put_owner),
    method("Window_get_Owner", 2, graph::get_owner),
    method("Window_put_MessageDrain", 2, graph::put_message_drain),
    method("Window_get_MessageDrain", 2, graph::get_message_drain),
    method("Window_get_BorderColor", 2, graph::get_border_color),
    method("Window_put_BorderColor", 2, graph::put_border_color),
    method("Window_get_FullScreenMode", 2, graph::get_full_screen_mode),
    method("Window_put_FullScreenMode", 2, graph::put_full_screen_mode),
    method(
        "Window_SetWindowForeground",
        2,
        graph::set_window_foreground,
    ),
    method("Window_NotifyOwnerMessage", 5, graph::notify_owner_message),
    method("Window_SetWindowPosition", 5, graph::set_window_position),
    method("Window_GetWindowPosition", 5, graph::get_window_position),
    method(
        "Window_GetMinIdealImageSize",
        3,
        graph::get_ideal_image_size,
    ),
    method(
        "Window_GetMaxIdealImageSize",
        3,
        graph::get_ideal_image_size,
    ),
    method("Window_GetRestorePosition", 5, graph::get_window_position),
    method("Window_HideCursor", 2, graph::hide_cursor),
    method("Window_IsCursorHidden", 2, graph::is_cursor_hidden),
    // IBasicVideo.
    method(
        "Video_get_AvgTimePerFrame",
        2,
        graph::get_avg_time_per_frame,
    ),
    method("Video_get_BitRate", 2, graph::get_bit_rate),
    method("Video_get_BitErrorRate", 2, graph::get_bit_error_rate),
    method("Video_get_VideoWidth", 2, graph::get_video_width),
    method("Video_get_VideoHeight", 2, graph::get_video_height),
    method("Video_put_SourceLeft", 2, graph::put_source_left),
    method("Video_get_SourceLeft", 2, graph::get_source_left),
    method("Video_put_SourceWidth", 2, graph::put_source_width),
    method("Video_get_SourceWidth", 2, graph::get_source_width),
    method("Video_put_SourceTop", 2, graph::put_source_top),
    method("Video_get_SourceTop", 2, graph::get_source_top),
    method("Video_put_SourceHeight", 2, graph::put_source_height),
    method("Video_get_SourceHeight", 2, graph::get_source_height),
    method("Video_put_DestinationLeft", 2, graph::put_dest_left),
    method("Video_get_DestinationLeft", 2, graph::get_dest_left),
    method("Video_put_DestinationWidth", 2, graph::put_dest_width),
    method("Video_get_DestinationWidth", 2, graph::get_dest_width),
    method("Video_put_DestinationTop", 2, graph::put_dest_top),
    method("Video_get_DestinationTop", 2, graph::get_dest_top),
    method("Video_put_DestinationHeight", 2, graph::put_dest_height),
    method("Video_get_DestinationHeight", 2, graph::get_dest_height),
    method("Video_SetSourcePosition", 5, graph::set_source_position),
    method("Video_GetSourcePosition", 5, graph::get_source_position),
    method(
        "Video_SetDefaultSourcePosition",
        1,
        graph::set_default_source,
    ),
    method("Video_SetDestinationPosition", 5, graph::set_dest_position),
    method("Video_GetDestinationPosition", 5, graph::get_dest_position),
    method(
        "Video_SetDefaultDestinationPosition",
        1,
        graph::set_default_dest,
    ),
    method("Video_GetVideoSize", 3, graph::get_video_size),
    method(
        "Video_GetVideoPaletteEntries",
        5,
        graph::get_video_palette_entries,
    ),
    method("Video_GetCurrentImage", 3, graph::get_current_image),
    method(
        "Video_IsUsingDefaultSource",
        1,
        graph::is_using_default_source,
    ),
    method(
        "Video_IsUsingDefaultDestination",
        1,
        graph::is_using_default_dest,
    ),
    // IBasicAudio.
    method("Audio_put_Volume", 2, graph::put_volume),
    method("Audio_get_Volume", 2, graph::get_volume),
    method("Audio_put_Balance", 2, graph::put_balance),
    method("Audio_get_Balance", 2, graph::get_balance),
    // IMediaEventSink, which the program's filters tell the graph their
    // events through.
    method("Sink_Notify", 4, graph::sink_notify),
    // The MPEG video decoder's IBaseFilter (IMediaFilter's and
    // IPersist's first), its output pin's IPin and IQualityControl.
    method("Decoder_GetClassID", 2, filter::decoder_get_class_id),
    method("Decoder_Stop", 1, s_ok),
    method("Decoder_Pause", 1, s_ok),
    method("Decoder_Run", 3, s_ok),
    method("Decoder_GetState", 3, filter::decoder_get_state),
    method("Decoder_SetSyncSource", 2, s_ok),
    method("Decoder_GetSyncSource", 2, filter::decoder_get_sync_source),
    method("Decoder_EnumPins", 2, e_notimpl),
    method("Decoder_FindPin", 3, filter::decoder_find_pin),
    method(
        "Decoder_QueryFilterInfo",
        2,
        filter::decoder_query_filter_info,
    ),
    method("Decoder_JoinFilterGraph", 3, s_ok),
    method("Decoder_QueryVendorInfo", 2, e_notimpl),
    method("Pin_Connect", 3, filter::pin_connect),
    method("Pin_ReceiveConnection", 3, e_unexpected),
    method("Pin_Disconnect", 1, filter::pin_disconnect),
    method("Pin_ConnectedTo", 2, filter::pin_connected_to),
    method(
        "Pin_ConnectionMediaType",
        2,
        filter::pin_connection_media_type,
    ),
    method("Pin_QueryPinInfo", 2, filter::pin_query_pin_info),
    method("Pin_QueryDirection", 2, filter::pin_query_direction),
    method("Pin_QueryId", 2, filter::pin_query_id),
    method("Pin_QueryAccept", 2, filter::pin_query_accept),
    method("Pin_EnumMediaTypes", 2, e_notimpl),
    method("Pin_QueryInternalConnections", 3, e_notimpl),
    method("Pin_EndOfStream", 1, e_unexpected),
    method("Pin_BeginFlush", 1, e_unexpected),
    method("Pin_EndFlush", 1, e_unexpected),
    method("Pin_NewSegment", 7, s_ok),
    method("Quality_Notify", 8, s_ok),
    method("Quality_SetSink", 2, s_ok),
    // The memory allocator (CLSID_MemoryAllocator) and its samples'
    // IMediaSample2 (IMediaSample's first).
    method("Alloc_SetProperties", 3, sample::set_properties),
    method("Alloc_GetProperties", 2, sample::get_properties),
    method("Alloc_Commit", 1, sample::commit),
    method("Alloc_Decommit", 1, sample::decommit),
    method("Alloc_GetBuffer", 5, sample::get_buffer),
    method("Alloc_ReleaseBuffer", 2, s_ok),
    method("MediaSample_GetPointer", 2, sample::get_pointer),
    method("MediaSample_GetSize", 1, sample::get_size),
    method("MediaSample_GetTime", 3, sample::get_time),
    method("MediaSample_SetTime", 3, sample::set_time),
    method("MediaSample_IsSyncPoint", 1, sample::is_sync_point),
    method("MediaSample_SetSyncPoint", 2, sample::set_sync_point),
    method("MediaSample_IsPreroll", 1, sample::is_preroll),
    method("MediaSample_SetPreroll", 2, sample::set_preroll),
    method("MediaSample_GetActualDataLength", 1, sample::get_actual),
    method("MediaSample_SetActualDataLength", 2, sample::set_actual),
    method("MediaSample_GetMediaType", 2, sample::get_media_type),
    method("MediaSample_SetMediaType", 2, sample::set_media_type),
    method("MediaSample_IsDiscontinuity", 1, sample::is_discontinuity),
    method("MediaSample_SetDiscontinuity", 2, sample::set_discontinuity),
    method("MediaSample_GetMediaTime", 3, sample::get_media_time),
    method("MediaSample_SetMediaTime", 3, sample::set_media_time),
    method(
        "MediaSample_GetProperties",
        3,
        sample::get_sample_properties,
    ),
    method(
        "MediaSample_SetProperties",
        3,
        sample::set_sample_properties,
    ),
    // The system clock (CLSID_SystemClock), the graph's reference clock.
    method("Clock_GetTime", 2, sample::clock_get_time),
    method("Clock_AdviseTime", 7, sample::clock_advise_time),
    method("Clock_AdvisePeriodic", 7, sample::clock_advise_periodic),
    method("Clock_Unadvise", 2, sample::clock_unadvise),
    // The decoder's streaming thread: a loop (`filter::stream_thread`).
    Api {
        name: "StreamThread",
        ordinal: 0,
        args: 1,
        conv: Conv::Loop,
        handler: filter::stream_thread,
    },
];

static QUARTZ_VTABLES: &[Vtable] = &[
    Vtable {
        name: "IGraphBuilder",
        methods: unknown![
            "Graph_AddFilter",
            "Graph_RemoveFilter",
            "Graph_EnumFilters",
            "Graph_FindFilterByName",
            "Graph_ConnectDirect",
            "Graph_Reconnect",
            "Graph_Disconnect",
            "Graph_SetDefaultSyncSource",
            "Graph_Connect",
            "Graph_Render",
            "Graph_RenderFile",
            "Graph_AddSourceFilter",
            "Graph_SetLogFile",
            "Graph_Abort",
            "Graph_ShouldOperationContinue",
        ],
    },
    Vtable {
        name: "IMediaControl",
        methods: dispatch![
            "Control_Run",
            "Control_Pause",
            "Control_Stop",
            "Control_GetState",
            "Control_RenderFile",
            "Control_AddSourceFilter",
            "Control_get_FilterCollection",
            "Control_get_RegFilterCollection",
            "Control_StopWhenReady",
        ],
    },
    Vtable {
        name: "IMediaEventEx",
        methods: dispatch![
            "Event_GetEventHandle",
            "Event_GetEvent",
            "Event_WaitForCompletion",
            "Event_CancelDefaultHandling",
            "Event_RestoreDefaultHandling",
            "Event_FreeEventParams",
            "Event_SetNotifyWindow",
            "Event_SetNotifyFlags",
            "Event_GetNotifyFlags",
        ],
    },
    Vtable {
        name: "IMediaPosition",
        methods: dispatch![
            "Position_get_Duration",
            "Position_put_CurrentPosition",
            "Position_get_CurrentPosition",
            "Position_get_StopTime",
            "Position_put_StopTime",
            "Position_get_PrerollTime",
            "Position_put_PrerollTime",
            "Position_put_Rate",
            "Position_get_Rate",
            "Position_CanSeekForward",
            "Position_CanSeekBackward",
        ],
    },
    Vtable {
        name: "IMediaSeeking",
        methods: unknown![
            "Seeking_GetCapabilities",
            "Seeking_CheckCapabilities",
            "Seeking_IsFormatSupported",
            "Seeking_QueryPreferredFormat",
            "Seeking_GetTimeFormat",
            "Seeking_IsUsingTimeFormat",
            "Seeking_SetTimeFormat",
            "Seeking_GetDuration",
            "Seeking_GetStopPosition",
            "Seeking_GetCurrentPosition",
            "Seeking_ConvertTimeFormat",
            "Seeking_SetPositions",
            "Seeking_GetPositions",
            "Seeking_GetAvailable",
            "Seeking_SetRate",
            "Seeking_GetRate",
            "Seeking_GetPreroll",
        ],
    },
    Vtable {
        name: "IVideoWindow",
        methods: dispatch![
            "Window_put_Caption",
            "Window_get_Caption",
            "Window_put_WindowStyle",
            "Window_get_WindowStyle",
            "Window_put_WindowStyleEx",
            "Window_get_WindowStyleEx",
            "Window_put_AutoShow",
            "Window_get_AutoShow",
            "Window_put_WindowState",
            "Window_get_WindowState",
            "Window_put_BackgroundPalette",
            "Window_get_BackgroundPalette",
            "Window_put_Visible",
            "Window_get_Visible",
            "Window_put_Left",
            "Window_get_Left",
            "Window_put_Width",
            "Window_get_Width",
            "Window_put_Top",
            "Window_get_Top",
            "Window_put_Height",
            "Window_get_Height",
            "Window_put_Owner",
            "Window_get_Owner",
            "Window_put_MessageDrain",
            "Window_get_MessageDrain",
            "Window_get_BorderColor",
            "Window_put_BorderColor",
            "Window_get_FullScreenMode",
            "Window_put_FullScreenMode",
            "Window_SetWindowForeground",
            "Window_NotifyOwnerMessage",
            "Window_SetWindowPosition",
            "Window_GetWindowPosition",
            "Window_GetMinIdealImageSize",
            "Window_GetMaxIdealImageSize",
            "Window_GetRestorePosition",
            "Window_HideCursor",
            "Window_IsCursorHidden",
        ],
    },
    Vtable {
        name: "IBasicVideo",
        methods: dispatch![
            "Video_get_AvgTimePerFrame",
            "Video_get_BitRate",
            "Video_get_BitErrorRate",
            "Video_get_VideoWidth",
            "Video_get_VideoHeight",
            "Video_put_SourceLeft",
            "Video_get_SourceLeft",
            "Video_put_SourceWidth",
            "Video_get_SourceWidth",
            "Video_put_SourceTop",
            "Video_get_SourceTop",
            "Video_put_SourceHeight",
            "Video_get_SourceHeight",
            "Video_put_DestinationLeft",
            "Video_get_DestinationLeft",
            "Video_put_DestinationWidth",
            "Video_get_DestinationWidth",
            "Video_put_DestinationTop",
            "Video_get_DestinationTop",
            "Video_put_DestinationHeight",
            "Video_get_DestinationHeight",
            "Video_SetSourcePosition",
            "Video_GetSourcePosition",
            "Video_SetDefaultSourcePosition",
            "Video_SetDestinationPosition",
            "Video_GetDestinationPosition",
            "Video_SetDefaultDestinationPosition",
            "Video_GetVideoSize",
            "Video_GetVideoPaletteEntries",
            "Video_GetCurrentImage",
            "Video_IsUsingDefaultSource",
            "Video_IsUsingDefaultDestination",
        ],
    },
    Vtable {
        name: "IBasicAudio",
        methods: dispatch![
            "Audio_put_Volume",
            "Audio_get_Volume",
            "Audio_put_Balance",
            "Audio_get_Balance",
        ],
    },
    Vtable {
        name: "IMediaEventSink",
        methods: unknown!["Sink_Notify"],
    },
    Vtable {
        name: "IBaseFilter",
        methods: unknown![
            "Decoder_GetClassID",
            "Decoder_Stop",
            "Decoder_Pause",
            "Decoder_Run",
            "Decoder_GetState",
            "Decoder_SetSyncSource",
            "Decoder_GetSyncSource",
            "Decoder_EnumPins",
            "Decoder_FindPin",
            "Decoder_QueryFilterInfo",
            "Decoder_JoinFilterGraph",
            "Decoder_QueryVendorInfo",
        ],
    },
    Vtable {
        name: "IPin",
        methods: unknown![
            "Pin_Connect",
            "Pin_ReceiveConnection",
            "Pin_Disconnect",
            "Pin_ConnectedTo",
            "Pin_ConnectionMediaType",
            "Pin_QueryPinInfo",
            "Pin_QueryDirection",
            "Pin_QueryId",
            "Pin_QueryAccept",
            "Pin_EnumMediaTypes",
            "Pin_QueryInternalConnections",
            "Pin_EndOfStream",
            "Pin_BeginFlush",
            "Pin_EndFlush",
            "Pin_NewSegment",
        ],
    },
    Vtable {
        name: "IQualityControl",
        methods: unknown!["Quality_Notify", "Quality_SetSink"],
    },
    Vtable {
        name: "IMemAllocator",
        methods: unknown![
            "Alloc_SetProperties",
            "Alloc_GetProperties",
            "Alloc_Commit",
            "Alloc_Decommit",
            "Alloc_GetBuffer",
            "Alloc_ReleaseBuffer",
        ],
    },
    Vtable {
        name: "IMediaSample2",
        methods: unknown![
            "MediaSample_GetPointer",
            "MediaSample_GetSize",
            "MediaSample_GetTime",
            "MediaSample_SetTime",
            "MediaSample_IsSyncPoint",
            "MediaSample_SetSyncPoint",
            "MediaSample_IsPreroll",
            "MediaSample_SetPreroll",
            "MediaSample_GetActualDataLength",
            "MediaSample_SetActualDataLength",
            "MediaSample_GetMediaType",
            "MediaSample_SetMediaType",
            "MediaSample_IsDiscontinuity",
            "MediaSample_SetDiscontinuity",
            "MediaSample_GetMediaTime",
            "MediaSample_SetMediaTime",
            "MediaSample_GetProperties",
            "MediaSample_SetProperties",
        ],
    },
    Vtable {
        name: "IReferenceClock",
        methods: unknown![
            "Clock_GetTime",
            "Clock_AdviseTime",
            "Clock_AdvisePeriodic",
            "Clock_Unadvise",
        ],
    },
];

/// AMSTREAM.DLL's functions. Append only (see `stubs`).
static AMSTREAM_APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object_amstream),
    stdcall("DllRegisterServer", 3, 0, s_ok),
    stdcall("DllUnregisterServer", 4, 0, s_ok),
    method(UNKNOWN[0], 3, query_interface),
    method(UNKNOWN[1], 1, add_ref),
    method(UNKNOWN[2], 1, release),
    // IAMMultiMediaStream (IMultiMediaStream's first).
    method("MMS_GetInformation", 3, stream::get_information),
    method("MMS_GetMediaStream", 3, stream::get_media_stream),
    method("MMS_EnumMediaStreams", 3, stream::enum_media_streams),
    method("MMS_GetState", 2, stream::get_state),
    method("MMS_SetState", 2, stream::set_state),
    method("MMS_GetTime", 2, stream::get_time),
    method("MMS_GetDuration", 2, stream::get_duration),
    method("MMS_Seek", 3, stream::seek),
    method(
        "MMS_GetEndOfStreamEventHandle",
        2,
        stream::get_end_of_stream_event,
    ),
    method("MMS_Initialize", 4, stream::initialize),
    method("MMS_GetFilterGraph", 2, stream::get_filter_graph),
    method("MMS_GetFilter", 2, e_notimpl),
    method("MMS_AddMediaStream", 5, stream::add_media_stream),
    method("MMS_OpenFile", 3, stream::open_file),
    method("MMS_OpenMoniker", 4, e_notimpl),
    method("MMS_Render", 2, stream::render),
    // IMediaStream.
    method(
        "Stream_GetMultiMediaStream",
        2,
        stream::get_multi_media_stream,
    ),
    method("Stream_GetInformation", 3, stream::stream_get_information),
    method("Stream_SetSameFormat", 3, stream::set_same_format),
    method("Stream_AllocateSample", 3, stream::allocate_sample),
    method("Stream_CreateSharedSample", 4, e_notimpl),
    method("Stream_SendEndOfStream", 2, stream::send_end_of_stream),
    // IDirectDrawMediaStream.
    method("DDStream_GetFormat", 5, stream::dd_get_format),
    method("DDStream_SetFormat", 3, stream::dd_set_format),
    method("DDStream_GetDirectDraw", 2, stream::dd_get_direct_draw),
    method("DDStream_SetDirectDraw", 2, stream::dd_set_direct_draw),
    method("DDStream_CreateSample", 5, stream::dd_create_sample),
    method("DDStream_GetTimePerFrame", 2, stream::dd_get_time_per_frame),
    // IAudioMediaStream.
    method("AudioStream_GetFormat", 2, stream::audio_get_format),
    method("AudioStream_SetFormat", 2, stream::audio_set_format),
    method("AudioStream_CreateSample", 4, stream::audio_create_sample),
    // IStreamSample.
    method("Sample_GetMediaStream", 2, stream::sample_get_media_stream),
    method("Sample_GetSampleTimes", 4, stream::get_sample_times),
    method("Sample_SetSampleTimes", 3, stream::set_sample_times),
    method("Sample_Update", 5, stream::update),
    method("Sample_CompletionStatus", 3, stream::completion_status),
    // IDirectDrawStreamSample.
    method("DDSample_GetSurface", 3, stream::get_surface),
    method("DDSample_SetRect", 2, stream::set_rect),
    // IAudioStreamSample.
    method("AudioSample_GetAudioData", 2, stream::get_audio_data),
    // IAudioData (IMemoryData's first).
    method("Data_SetBuffer", 4, stream::data_set_buffer),
    method("Data_GetInfo", 4, stream::data_get_info),
    method("Data_SetActual", 2, stream::data_set_actual),
    method("Data_GetFormat", 2, stream::data_get_format),
    method("Data_SetFormat", 2, stream::data_set_format),
];

/// IMediaStream's methods, which the media streams' interfaces start
/// with.
macro_rules! media_stream {
    ($($m:literal),* $(,)?) => {
        unknown![
            "Stream_GetMultiMediaStream",
            "Stream_GetInformation",
            "Stream_SetSameFormat",
            "Stream_AllocateSample",
            "Stream_CreateSharedSample",
            "Stream_SendEndOfStream",
            $($m),*
        ]
    };
}

/// IStreamSample's methods, which the samples' interfaces start with.
macro_rules! stream_sample {
    ($($m:literal),* $(,)?) => {
        unknown![
            "Sample_GetMediaStream",
            "Sample_GetSampleTimes",
            "Sample_SetSampleTimes",
            "Sample_Update",
            "Sample_CompletionStatus",
            $($m),*
        ]
    };
}

static AMSTREAM_VTABLES: &[Vtable] = &[
    Vtable {
        name: "IAMMultiMediaStream",
        methods: unknown![
            "MMS_GetInformation",
            "MMS_GetMediaStream",
            "MMS_EnumMediaStreams",
            "MMS_GetState",
            "MMS_SetState",
            "MMS_GetTime",
            "MMS_GetDuration",
            "MMS_Seek",
            "MMS_GetEndOfStreamEventHandle",
            "MMS_Initialize",
            "MMS_GetFilterGraph",
            "MMS_GetFilter",
            "MMS_AddMediaStream",
            "MMS_OpenFile",
            "MMS_OpenMoniker",
            "MMS_Render",
        ],
    },
    Vtable {
        name: "IDirectDrawMediaStream",
        methods: media_stream![
            "DDStream_GetFormat",
            "DDStream_SetFormat",
            "DDStream_GetDirectDraw",
            "DDStream_SetDirectDraw",
            "DDStream_CreateSample",
            "DDStream_GetTimePerFrame",
        ],
    },
    Vtable {
        name: "IAudioMediaStream",
        methods: media_stream![
            "AudioStream_GetFormat",
            "AudioStream_SetFormat",
            "AudioStream_CreateSample",
        ],
    },
    Vtable {
        name: "IDirectDrawStreamSample",
        methods: stream_sample!["DDSample_GetSurface", "DDSample_SetRect"],
    },
    Vtable {
        name: "IAudioStreamSample",
        methods: stream_sample!["AudioSample_GetAudioData"],
    },
    Vtable {
        name: "IAudioData",
        methods: unknown![
            "Data_SetBuffer",
            "Data_GetInfo",
            "Data_SetActual",
            "Data_GetFormat",
            "Data_SetFormat",
        ],
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

pub const GRAPH_BUILDER: Iface = Iface(D_QUARTZ, 0);
pub const MEDIA_CONTROL: Iface = Iface(D_QUARTZ, 1);
pub const MEDIA_EVENT: Iface = Iface(D_QUARTZ, 2);
pub const MEDIA_POSITION: Iface = Iface(D_QUARTZ, 3);
pub const MEDIA_SEEKING: Iface = Iface(D_QUARTZ, 4);
pub const VIDEO_WINDOW: Iface = Iface(D_QUARTZ, 5);
pub const BASIC_VIDEO: Iface = Iface(D_QUARTZ, 6);
pub const BASIC_AUDIO: Iface = Iface(D_QUARTZ, 7);
pub const EVENT_SINK: Iface = Iface(D_QUARTZ, 8);
pub const DECODER: Iface = Iface(D_QUARTZ, 9);
pub const PIN: Iface = Iface(D_QUARTZ, 10);
pub const QUALITY: Iface = Iface(D_QUARTZ, 11);
pub const ALLOCATOR: Iface = Iface(D_QUARTZ, 12);
pub const MEDIA_SAMPLE: Iface = Iface(D_QUARTZ, 13);
pub const CLOCK: Iface = Iface(D_QUARTZ, 14);
pub const MULTI_STREAM: Iface = Iface(D_AMSTREAM, 0);
pub const DD_STREAM: Iface = Iface(D_AMSTREAM, 1);
pub const AUDIO_STREAM: Iface = Iface(D_AMSTREAM, 2);
pub const DD_SAMPLE: Iface = Iface(D_AMSTREAM, 3);
pub const AUDIO_SAMPLE: Iface = Iface(D_AMSTREAM, 4);
pub const AUDIO_DATA: Iface = Iface(D_AMSTREAM, 5);

/// The classes CoCreateInstance makes of DirectShow's (com.rs).
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: CLSID_FILTER_GRAPH,
        dll: "QUARTZ.DLL",
        name: "Filter Graph",
        create: graph::create,
    },
    com::Class {
        clsid: CLSID_FILTER_GRAPH_NO_THREAD,
        dll: "QUARTZ.DLL",
        name: "Filter Graph no thread",
        create: graph::create,
    },
    com::Class {
        clsid: CLSID_MEMORY_ALLOCATOR,
        dll: "QUARTZ.DLL",
        name: "Memory Allocator",
        create: sample::create_allocator,
    },
    com::Class {
        clsid: CLSID_SYSTEM_CLOCK,
        dll: "QUARTZ.DLL",
        name: "System Clock",
        create: sample::create_clock,
    },
    com::Class {
        clsid: CLSID_AM_MULTI_MEDIA_STREAM,
        dll: "AMSTREAM.DLL",
        name: "AMMultiMediaStream",
        create: stream::create,
    },
    com::Class {
        clsid: CLSID_AM_AUDIO_DATA,
        dll: "AMSTREAM.DLL",
        name: "AMAudioData",
        create: stream::create_audio_data,
    },
];

// The objects.

/// An object of DirectShow's.
#[derive(Clone, Debug, Default)]
pub struct Object {
    pub id: u32,
    pub pid: u32,
    pub refs: u32,
    /// Its interfaces' blocks so far: (`Iface::key`, address).
    pub ifaces: Vec<(u32, u32)>,
    pub kind: Kind,
}

/// What an object is, and its state.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Kind {
    Graph { g: graph::Graph },
    Multi { m: stream::Multi },
    Video { v: stream::Video },
    Audio { a: stream::Audio },
    VideoSample { s: stream::Sample },
    AudioSample { s: stream::Sample },
    AudioData { d: stream::AudioData },
    Decoder { d: filter::Decoder },
    Pin { p: filter::Pin },
    Allocator { a: sample::Allocator },
    MediaSample { s: sample::MediaSample },
    Clock { c: sample::Clock },
}

impl Default for Kind {
    fn default() -> Kind {
        Kind::AudioData {
            d: stream::AudioData::default(),
        }
    }
}

impl Kind {
    /// Its interfaces: what QueryInterface gives for each IID. The first
    /// is the one IUnknown is.
    fn interfaces(&self) -> &'static [(Guid, Iface)] {
        match self {
            Kind::Graph { .. } => &[
                (IID_IGRAPH_BUILDER, GRAPH_BUILDER),
                (IID_IFILTER_GRAPH, GRAPH_BUILDER),
                (IID_IMEDIA_CONTROL, MEDIA_CONTROL),
                (IID_IMEDIA_EVENT_EX, MEDIA_EVENT),
                (IID_IMEDIA_EVENT, MEDIA_EVENT),
                (IID_IMEDIA_POSITION, MEDIA_POSITION),
                (IID_IMEDIA_SEEKING, MEDIA_SEEKING),
                (IID_IVIDEO_WINDOW, VIDEO_WINDOW),
                (IID_IBASIC_VIDEO, BASIC_VIDEO),
                (IID_IBASIC_AUDIO, BASIC_AUDIO),
                (IID_IMEDIA_EVENT_SINK, EVENT_SINK),
            ],
            Kind::Decoder { .. } => &[
                (IID_IBASE_FILTER, DECODER),
                (IID_IMEDIA_FILTER, DECODER),
                (IID_IPERSIST, DECODER),
            ],
            Kind::Pin { .. } => &[(IID_IPIN, PIN), (IID_IQUALITY_CONTROL, QUALITY)],
            Kind::Allocator { .. } => &[(IID_IMEM_ALLOCATOR, ALLOCATOR)],
            Kind::MediaSample { .. } => &[
                (IID_IMEDIA_SAMPLE, MEDIA_SAMPLE),
                (IID_IMEDIA_SAMPLE2, MEDIA_SAMPLE),
            ],
            Kind::Clock { .. } => &[(IID_IREFERENCE_CLOCK, CLOCK)],
            Kind::Multi { .. } => &[
                (IID_IAM_MULTI_MEDIA_STREAM, MULTI_STREAM),
                (IID_IMULTI_MEDIA_STREAM, MULTI_STREAM),
            ],
            Kind::Video { .. } => &[
                (IID_IMEDIA_STREAM, DD_STREAM),
                (IID_IDIRECT_DRAW_MEDIA_STREAM, DD_STREAM),
            ],
            Kind::Audio { .. } => &[
                (IID_IMEDIA_STREAM, AUDIO_STREAM),
                (IID_IAUDIO_MEDIA_STREAM, AUDIO_STREAM),
            ],
            Kind::VideoSample { .. } => &[
                (IID_ISTREAM_SAMPLE, DD_SAMPLE),
                (IID_IDIRECT_DRAW_STREAM_SAMPLE, DD_SAMPLE),
            ],
            Kind::AudioSample { .. } => &[
                (IID_ISTREAM_SAMPLE, AUDIO_SAMPLE),
                (IID_IAUDIO_STREAM_SAMPLE, AUDIO_SAMPLE),
            ],
            Kind::AudioData { .. } => &[
                (IID_IAUDIO_DATA, AUDIO_DATA),
                (IID_IMEMORY_DATA, AUDIO_DATA),
            ],
        }
    }
}

/// DirectShow's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub objects: Vec<Object>,
    pub last_id: u32,
}

pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.dshow
}

impl State {
    pub fn get(&self, id: u32) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }
}

/// The kind of the object `id`, if there is one.
pub fn kind(cpu: &mut Cpu, id: u32) -> Option<&mut Kind> {
    state(cpu).get_mut(id).map(|o| &mut o.kind)
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
        kind,
    });
    id
}

/// Map the DLL `name` of the two in the running process, with the DLLs
/// whose code its objects run in the program's process. QUARTZ.DLL
/// imports USER32, GDI32 and WINMM, and its video renderer's window
/// procedure is USER32's here. AMSTREAM.DLL loads QUARTZ.DLL, and its
/// samples' surfaces are DirectDraw's, whose methods are in DDRAW.DLL.
fn map_dll(cpu: &mut Cpu, name: &str) -> Option<u32> {
    let deps: &[&str] = if name == QUARTZ.name {
        &["USER32.DLL", "GDI32.DLL", "WINMM.DLL"]
    } else {
        &["USER32.DLL", "GDI32.DLL", "WINMM.DLL", "DDRAW.DLL"]
    };
    for dep in deps {
        super::loader::map_builtin_named(cpu, dep)?;
    }
    super::loader::map_builtin_named(cpu, name)
}

/// Take a reference on the object `id`.
pub fn hold(cpu: &mut Cpu, id: u32) {
    if let Some(o) = state(cpu).get_mut(id) {
        o.refs += 1;
    }
}

/// Give back a reference on the object `id`: its count left. With none
/// left it goes, and what it held is let go.
pub fn drop_ref(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(o) = state(cpu).get_mut(id) else {
        return 0;
    };
    o.refs = o.refs.saturating_sub(1);
    if o.refs > 0 {
        return o.refs;
    }
    match &o.kind {
        Kind::Graph { .. } => graph::close(cpu, id),
        Kind::Multi { .. } => stream::close(cpu, id),
        // A sample goes back to its allocator, which keeps it.
        Kind::MediaSample { .. } => {
            sample::returned(cpu, id);
            return 0;
        }
        Kind::Allocator { .. } => sample::free_allocator(cpu, id),
        Kind::Clock { .. } => sample::close_clock(cpu, id),
        _ => {}
    }
    let s = state(cpu);
    let Some(i) = s.objects.iter().position(|o| o.id == id) else {
        return 0;
    };
    let gone = s.objects.remove(i);
    for &(_, at) in &gone.ifaces {
        com::free_interface(cpu, at);
    }
    stream::let_go(cpu, &gone.kind);
    0
}

/// The object `id` goes, whatever its count, with its interfaces: for
/// an allocator's samples, which it frees with itself.
pub fn forget(cpu: &mut Cpu, id: u32) {
    let s = state(cpu);
    let Some(i) = s.objects.iter().position(|o| o.id == id) else {
        return;
    };
    let gone = s.objects.remove(i);
    for &(_, at) in &gone.ifaces {
        com::free_interface(cpu, at);
    }
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
    // object through the other DLL hasn't mapped yet.
    map_dll(cpu, name)?;
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
    let found = if *iid == com::IID_IUNKNOWN {
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

/// The first interface of the object `id` (its own), to give the
/// program, with a reference taken; 0 when the heap is full.
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

/// A process ended: its objects go. Its windows, sounds, handles and
/// MCI devices go with the process, by their own modules.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let events: Vec<u32> = state(cpu)
        .objects
        .iter()
        .filter(|o| o.pid == pid)
        .flat_map(|o| match &o.kind {
            Kind::Graph { g } => {
                let mut held = vec![g.event.1];
                held.extend([&g.link, &g.sound].into_iter().flatten().map(|l| l.wake));
                held
            }
            Kind::Multi { m } => vec![m.end_event.1],
            Kind::VideoSample { s } | Kind::AudioSample { s } => vec![s.event],
            Kind::Clock { c } => c.advises.iter().map(|a| a.object).collect(),
            Kind::Allocator { a } => vec![a.freed],
            _ => Vec::new(),
        })
        .filter(|&e| e != 0)
        .collect();
    state(cpu).objects.retain(|o| o.pid != pid);
    for e in events {
        super::object::objects(cpu).release(e);
    }
}

/// A graph's play ended: EC_COMPLETE (winmm/mci).
pub fn play_ended(cpu: &mut Cpu, graph: u32) {
    graph::play_ended(cpu, graph);
}

/// The process `pid`'s multimedia streams do what is due at `now`
/// (microseconds), on its multimedia thread (winmm/callback.rs): their
/// end-of-stream events and their samples' updates.
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    stream::poll(cpu, pid, now);
    sample::poll(cpu, pid, now);
}

/// When the process `pid`'s multimedia streams next have something due,
/// or its clocks an advise.
pub fn next_due(cpu: &Cpu, pid: Option<u32>) -> Option<u64> {
    let s = &cpu.bus.win32.dshow;
    [stream::next_due(s, pid), sample::next_due(s, pid)]
        .into_iter()
        .flatten()
        .min()
}

/// Set the event object `id` (by its id, not a handle), and wake what
/// waits for it.
pub fn set_event(cpu: &mut Cpu, id: u32) {
    if id != 0 {
        super::winmm::callback::set_event(cpu, id);
        super::thread::wake(cpu);
    }
}

/// Reset the event object `id`.
pub fn reset_event(cpu: &mut Cpu, id: u32) {
    if let Some(o) = super::object::objects(cpu).get_mut(id)
        && let super::object::Kind::Event { signaled, .. } = &mut o.kind
    {
        *signaled = false;
    }
}

/// A new manual-reset event, not set: its handle in the running process
/// and its object, which the caller holds once more besides the handle.
pub fn new_event(cpu: &mut Cpu) -> (u32, u32) {
    let kind = super::object::Kind::Event {
        manual: true,
        signaled: false,
    };
    let handle = super::object::create(cpu, kind, None);
    let id = super::object::resolve(cpu, handle).unwrap_or(0);
    if id != 0 {
        super::object::objects(cpu).hold(id);
    }
    (handle, id)
}

/// Let go of an event `new_event` made: its handle and the hold.
pub fn free_event(cpu: &mut Cpu, (handle, id): (u32, u32)) {
    if id == 0 {
        return;
    }
    if super::object::resolve(cpu, handle) == Some(id) {
        super::object::close(cpu, handle);
    }
    super::object::objects(cpu).release(id);
}

/// DllCanUnloadNow: S_FALSE while the process has objects.
fn dll_can_unload_now(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let pid = super::user32::current_pid(cpu);
    let used = state(cpu).objects.iter().any(|o| o.pid == pid);
    Some(if used { S_FALSE } else { S_OK })
}

fn dll_get_class_object_quartz(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, QUARTZ.name, args))
}

fn dll_get_class_object_amstream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, AMSTREAM.name, args))
}

fn s_ok(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

fn e_notimpl(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

fn e_unexpected(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_UNEXPECTED)
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
    // A graph's last reference: the program's filters in it are told
    // first, which calls into the program.
    if state(cpu).get(id).is_some_and(|o| o.refs == 1) && filter::has_filters(cpu, id) {
        return filter::close(cpu, id);
    }
    Some(drop_ref(cpu, id))
}

/// Write `value` at the program's pointer `at`: E_POINTER if it can't be
/// written there.
pub fn put32(cpu: &mut Cpu, at: u32, value: u32) -> Result<(), u32> {
    if com::write_out(cpu, at, value) {
        Ok(())
    } else {
        Err(E_POINTER)
    }
}

/// Write a 64-bit `value` at the program's pointer `at`.
pub fn put64(cpu: &mut Cpu, at: u32, value: u64) -> Result<(), u32> {
    if at == 0 || !cpu.bus.guest_probe(at, 8, true) {
        return Err(E_POINTER);
    }
    cpu.bus.guest_write_bytes(at, &value.to_le_bytes());
    Ok(())
}

/// The 64-bit value at the program's pointer `at`.
pub fn get64(cpu: &mut Cpu, at: u32) -> Result<u64, u32> {
    if at == 0 || !cpu.bus.guest_probe(at, 8, false) {
        return Err(E_POINTER);
    }
    let mut b = [0; 8];
    cpu.bus.guest_read_bytes(at, &mut b);
    Ok(u64::from_le_bytes(b))
}

crate::state_fields!(Object {
    id,
    pid,
    refs,
    ifaces,
    kind
});
super::state_variants!(Kind {
    Graph { g },
    Multi { m },
    Video { v },
    Audio { a },
    VideoSample { s },
    AudioSample { s },
    AudioData { d },
    Decoder { d },
    Pin { p },
    Allocator { a },
    MediaSample { s },
    Clock { c },
});
crate::state_fields!(State { objects, last_id });
