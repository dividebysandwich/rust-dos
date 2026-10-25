//! DirectMusic's GUIDs: its classes, interfaces, parameters and
//! notifications, as DirectX 6.1's headers have them.

use crate::win32::com::Guid;

/// The GUIDs of DirectMusic that end in 8704-00600893B1BD.
const fn dm(d1: u32) -> Guid {
    Guid::new(
        d1,
        0xB39B,
        0x11D1,
        [0x87, 0x04, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
    )
}

pub const CLSID_DIRECTMUSIC: Guid = Guid::new(
    0x636B_9F10,
    0x0C7D,
    0x11D1,
    [0x95, 0xB2, 0x00, 0x20, 0xAF, 0xDC, 0x74, 0x21],
);
pub const CLSID_COLLECTION: Guid = Guid::new(
    0x480F_F4B0,
    0x28B2,
    0x11D1,
    [0xBE, 0xF7, 0x00, 0xC0, 0x4F, 0xBF, 0x8F, 0xEF],
);
pub const CLSID_SYNTH: Guid = Guid::new(
    0x58C2_B4D0,
    0x46E7,
    0x11D1,
    [0x89, 0xAC, 0x00, 0xA0, 0xC9, 0x05, 0x41, 0x29],
);
pub const CLSID_PERFORMANCE: Guid = dm(0xD2AC_2881);
pub const CLSID_SEGMENT: Guid = dm(0xD2AC_2882);
pub const CLSID_SEGMENT_STATE: Guid = dm(0xD2AC_2883);
pub const CLSID_GRAPH: Guid = dm(0xD2AC_2884);
pub const CLSID_STYLE: Guid = dm(0xD2AC_288A);
pub const CLSID_CHORD_MAP: Guid = dm(0xD2AC_288F);
pub const CLSID_COMPOSER: Guid = dm(0xD2AC_2890);
pub const CLSID_LOADER: Guid = dm(0xD2AC_2892);
pub const CLSID_BAND: Guid = Guid::new(
    0x79BA_9E00,
    0xB6EE,
    0x11D1,
    [0x86, 0xBE, 0x00, 0xC0, 0x4F, 0xBF, 0x8F, 0xEF],
);

/// The tracks' classes.
pub const CLSID_TEMPO_TRACK: Guid = dm(0xD2AC_2885);
pub const CLSID_SEQ_TRACK: Guid = dm(0xD2AC_2886);
pub const CLSID_SYSEX_TRACK: Guid = dm(0xD2AC_2887);
pub const CLSID_TIME_SIG_TRACK: Guid = dm(0xD2AC_2888);
pub const CLSID_CHORD_TRACK: Guid = dm(0xD2AC_288B);
pub const CLSID_COMMAND_TRACK: Guid = dm(0xD2AC_288C);
pub const CLSID_STYLE_TRACK: Guid = dm(0xD2AC_288D);
pub const CLSID_MOTIF_TRACK: Guid = dm(0xD2AC_288E);
pub const CLSID_BAND_TRACK: Guid = dm(0xD2AC_2894);
pub const CLSID_CHORD_MAP_TRACK: Guid = dm(0xD2AC_2896);
pub const CLSID_MUTE_TRACK: Guid = dm(0xD2AC_2898);
pub const CLSID_SIGN_POST_TRACK: Guid = Guid::new(
    0xF17E_8672,
    0xC3B4,
    0x11D1,
    [0x87, 0x0B, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
);

pub const IID_IUNKNOWN: Guid = crate::win32::com::IID_IUNKNOWN;
pub const IID_IPERSIST: Guid = Guid::new(0x10C, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IPERSIST_STREAM: Guid = Guid::new(0x109, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IREFERENCE_CLOCK: Guid = Guid::new(
    0x56A8_6897,
    0x0AD4,
    0x11CE,
    [0xB0, 0x3A, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
);
pub const IID_IDIRECTMUSIC: Guid = Guid::new(
    0x6536_115A,
    0x7B2D,
    0x11D2,
    [0xBA, 0x18, 0x00, 0x00, 0xF8, 0x75, 0xAC, 0x12],
);
pub const IID_IDIRECTMUSIC2: Guid = Guid::new(
    0x6FC2_CAE1,
    0xBC78,
    0x11D2,
    [0xAF, 0xA6, 0x00, 0xAA, 0x00, 0x24, 0xD8, 0xB6],
);
pub const IID_IDIRECTMUSIC_PORT: Guid = Guid::new(
    0x08F2_D8C9,
    0x37C2,
    0x11D2,
    [0xB9, 0xF9, 0x00, 0x00, 0xF8, 0x75, 0xAC, 0x12],
);
pub const IID_IDIRECTMUSIC_BUFFER: Guid = dm(0xD2AC_2878);
pub const IID_IDIRECTMUSIC_COLLECTION: Guid = dm(0xD2AC_287C);
pub const IID_IDIRECTMUSIC_INSTRUMENT: Guid = dm(0xD2AC_287D);
pub const IID_IDIRECTMUSIC_DOWNLOADED_INSTRUMENT: Guid = dm(0xD2AC_287E);
pub const IID_IDIRECTMUSIC_OBJECT: Guid = dm(0xD2AC_28B5);
pub const IID_IDIRECTMUSIC_STYLE: Guid = dm(0xD2AC_28BD);
pub const IID_IDIRECTMUSIC_CHORD_MAP: Guid = dm(0xD2AC_28BE);
pub const IID_IDIRECTMUSIC_COMPOSER: Guid = dm(0xD2AC_28BF);
pub const IID_IDIRECTMUSIC_BAND: Guid = dm(0xD2AC_28C0);
pub const IID_IDIRECTMUSIC_GRAPH: Guid = Guid::new(
    0x2BEF_C277,
    0x5497,
    0x11D2,
    [0xBC, 0xCB, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);
pub const IID_IDIRECTMUSIC_LOADER: Guid = Guid::new(
    0x2FFA_ACA2,
    0x5DCA,
    0x11D2,
    [0xAF, 0xA6, 0x00, 0xAA, 0x00, 0x24, 0xD8, 0xB6],
);
pub const IID_IDIRECTMUSIC_PERFORMANCE: Guid = Guid::new(
    0x07D4_3D03,
    0x6523,
    0x11D2,
    [0x87, 0x1D, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
);
pub const IID_IDIRECTMUSIC_SEGMENT: Guid = Guid::new(
    0xF960_29A2,
    0x4282,
    0x11D2,
    [0x87, 0x17, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
);
pub const IID_IDIRECTMUSIC_SEGMENT_STATE: Guid = Guid::new(
    0xA3AF_DCC7,
    0xD3EE,
    0x11D1,
    [0xBC, 0x8D, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);
pub const IID_IDIRECTMUSIC_TRACK: Guid = Guid::new(
    0xF960_29A1,
    0x4282,
    0x11D2,
    [0x87, 0x17, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
);

/// The types of objects GetObject and the search directories take: all.
pub const GUID_ALL_TYPES: Guid = dm(0xD2AC_2893);
pub const GUID_DEFAULT_GM_COLLECTION: Guid = Guid::new(
    0xF17E_8673,
    0xC3B4,
    0x11D1,
    [0x87, 0x0B, 0x00, 0x60, 0x08, 0x93, 0xB1, 0xBD],
);

/// The parameters of GetParam and SetParam.
pub const GUID_TEMPO_PARAM: Guid = dm(0xD2AC_28A5);
pub const GUID_TIME_SIGNATURE: Guid = dm(0xD2AC_28A4);
pub const GUID_DOWNLOAD: Guid = dm(0xD2AC_28A7);
pub const GUID_UNLOAD: Guid = dm(0xD2AC_28A8);
pub const GUID_ENABLE_AUTO_DOWNLOAD: Guid = dm(0xD2AC_28A9);
pub const GUID_DISABLE_AUTO_DOWNLOAD: Guid = dm(0xD2AC_28AA);
pub const GUID_CLEAR_ALL_BANDS: Guid = dm(0xD2AC_28AB);
pub const GUID_IDIRECTMUSIC_BAND: Guid = dm(0xD2AC_28AC);
pub const GUID_MUTE_PARAM: Guid = dm(0xD2AC_28AF);
pub const GUID_STANDARD_MIDI_FILE: Guid = Guid::new(
    0x0662_1075,
    0xE92E,
    0x11D1,
    [0xA8, 0xC5, 0x00, 0xC0, 0x4F, 0xA3, 0x72, 0x6E],
);
pub const GUID_CONNECT_TO_DLS_COLLECTION: Guid = Guid::new(
    0x1DB1_AE6B,
    0xE92E,
    0x11D1,
    [0xA8, 0xC5, 0x00, 0xC0, 0x4F, 0xA3, 0x72, 0x6E],
);
pub const GUID_DISABLE_TEMPO: Guid = Guid::new(
    0x45FC_707D,
    0x1DB4,
    0x11D2,
    [0xBC, 0xAC, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);
pub const GUID_ENABLE_TEMPO: Guid = Guid::new(
    0x45FC_707E,
    0x1DB4,
    0x11D2,
    [0xBC, 0xAC, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);
pub const GUID_DISABLE_TIME_SIG: Guid = Guid::new(
    0x45FC_707B,
    0x1DB4,
    0x11D2,
    [0xBC, 0xAC, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);
pub const GUID_ENABLE_TIME_SIG: Guid = Guid::new(
    0x45FC_707C,
    0x1DB4,
    0x11D2,
    [0xBC, 0xAC, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);

/// The performance's global parameters.
pub const GUID_PERF_MASTER_TEMPO: Guid = dm(0xD2AC_28B0);
pub const GUID_PERF_MASTER_VOLUME: Guid = dm(0xD2AC_28B1);
pub const GUID_PERF_AUTO_DOWNLOAD: Guid = Guid::new(
    0xFB09_565B,
    0x3631,
    0x11D2,
    [0xBC, 0xB8, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);

/// The notifications.
pub const GUID_NOTIFICATION_SEGMENT: Guid = dm(0xD2AC_2899);
pub const GUID_NOTIFICATION_MEASURE_AND_BEAT: Guid = dm(0xD2AC_289A);
pub const GUID_NOTIFICATION_PERFORMANCE: Guid = Guid::new(
    0x81F7_5BC5,
    0x4E5D,
    0x11D2,
    [0xBC, 0xC7, 0x00, 0xA0, 0xC9, 0x22, 0xE6, 0xEB],
);

/// The ports and the clock DirectMusic lists.
pub const GUID_MIDI_MAPPER_PORT: Guid = Guid::new(
    0x4194_1661,
    0x73FC,
    0x11CE,
    [0x92, 0xF6, 0xA7, 0xB1, 0x9F, 0x95, 0x62, 0x2E],
);
pub const GUID_SYSTEM_CLOCK: Guid = Guid::new(
    0x58D5_8419,
    0x71B4,
    0x11D1,
    [0xA7, 0x4C, 0x00, 0x00, 0xF8, 0x75, 0xAC, 0x12],
);
