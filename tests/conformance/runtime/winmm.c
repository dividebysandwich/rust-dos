/*
 * What Wine 1.2.3's winmm tests other than wave.c and capture.c take
 * from wave.c, which Wine builds into one program with them: the names
 * of the multimedia errors, for their messages. Linked with each of
 * them by conformance_tests.rs.
 *
 * Part of rust-dos, GPL-2.0-or-later.
 */

#include <windows.h>
#include <mmsystem.h>
#include <stdio.h>

const char *mmsys_error(MMRESULT error)
{
    static char unknown[32];
#define ERR_TO_STR(e) case e: return #e
    switch (error) {
    ERR_TO_STR(MMSYSERR_NOERROR);
    ERR_TO_STR(MMSYSERR_ERROR);
    ERR_TO_STR(MMSYSERR_BADDEVICEID);
    ERR_TO_STR(MMSYSERR_NOTENABLED);
    ERR_TO_STR(MMSYSERR_ALLOCATED);
    ERR_TO_STR(MMSYSERR_INVALHANDLE);
    ERR_TO_STR(MMSYSERR_NODRIVER);
    ERR_TO_STR(MMSYSERR_NOMEM);
    ERR_TO_STR(MMSYSERR_NOTSUPPORTED);
    ERR_TO_STR(MMSYSERR_BADERRNUM);
    ERR_TO_STR(MMSYSERR_INVALFLAG);
    ERR_TO_STR(MMSYSERR_INVALPARAM);
    ERR_TO_STR(MIDIERR_UNPREPARED);
    ERR_TO_STR(MIDIERR_STILLPLAYING);
    ERR_TO_STR(MIDIERR_NOTREADY);
    ERR_TO_STR(MIDIERR_NODEVICE);
    ERR_TO_STR(MIDIERR_INVALIDSETUP);
    ERR_TO_STR(TIMERR_NOCANDO);
    ERR_TO_STR(TIMERR_STRUCT);
    ERR_TO_STR(MIXERR_INVALLINE);
    ERR_TO_STR(MIXERR_INVALCONTROL);
    ERR_TO_STR(MIXERR_INVALVALUE);
    ERR_TO_STR(MMIOERR_FILENOTFOUND);
    ERR_TO_STR(MMIOERR_CANNOTOPEN);
    ERR_TO_STR(MMIOERR_CANNOTSEEK);
    ERR_TO_STR(MMIOERR_CHUNKNOTFOUND);
    ERR_TO_STR(MMIOERR_UNBUFFERED);
    }
#undef ERR_TO_STR
    sprintf(unknown, "Unknown(0x%08x)", error);
    return unknown;
}
