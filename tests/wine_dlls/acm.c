/* Wine's MSACM32.DLL and its IMA ADPCM codec (imaadp32.acm), run as
 * guest code (tests/wine_dlls.rs): 16-bit PCM is encoded to IMA ADPCM
 * and decoded back through acmStreamOpen and acmStreamConvert, and the
 * decoded samples are checked against a decoder of this program's own.
 * Each line it prints is a result; the last says whether all held. */

#include <windows.h>
#include <mmreg.h>
#include <msacm.h>
#include <stdio.h>
#include <string.h>

#define BLOCK 256
#define PER_BLOCK 505
#define BLOCKS 4
#define SAMPLES (PER_BLOCK * BLOCKS)

static const int steps[89] = {
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37,
    41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173,
    190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484,
    7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818,
    18500, 20350, 22385, 24623, 27086, 29794, 32767};
static const int index_change[16] = {-1, -1, -1, -1, 2, 4, 6, 8,
                                     -1, -1, -1, -1, 2, 4, 6, 8};

static int failures;

static void check(int ok, const char *what)
{
    printf("%s: %s\n", what, ok ? "ok" : "FAILED");
    if (!ok)
        failures++;
}

/* One mono IMA ADPCM block, as the format defines it. */
static void decode_block(const BYTE *in, short *out)
{
    int sample = (short)(in[0] | in[1] << 8);
    int index = in[2];
    int i;
    *out++ = (short)sample;
    for (i = 4; i < BLOCK; i++) {
        int half;
        for (half = 0; half < 2; half++) {
            int code = half ? in[i] >> 4 : in[i] & 15;
            int step = steps[index];
            int diff = step >> 3;
            if (code & 1) diff += step >> 2;
            if (code & 2) diff += step >> 1;
            if (code & 4) diff += step;
            sample += code & 8 ? -diff : diff;
            if (sample > 32767) sample = 32767;
            if (sample < -32768) sample = -32768;
            index += index_change[code];
            if (index < 0) index = 0;
            if (index > 88) index = 88;
            *out++ = (short)sample;
        }
    }
}

static MMRESULT convert(WAVEFORMATEX *from, WAVEFORMATEX *to, void *src,
                        DWORD src_len, void *dst, DWORD dst_len, DWORD *used)
{
    HACMSTREAM stream;
    ACMSTREAMHEADER header;
    MMRESULT r = acmStreamOpen(&stream, NULL, from, to, NULL, 0, 0,
                               ACM_STREAMOPENF_NONREALTIME);
    if (r)
        return r;
    memset(&header, 0, sizeof header);
    header.cbStruct = sizeof header;
    header.pbSrc = src;
    header.cbSrcLength = src_len;
    header.pbDst = dst;
    header.cbDstLength = dst_len;
    r = acmStreamPrepareHeader(stream, &header, 0);
    if (!r)
        r = acmStreamConvert(stream, &header, ACM_STREAMCONVERTF_BLOCKALIGN);
    *used = header.cbDstLengthUsed;
    acmStreamUnprepareHeader(stream, &header, 0);
    acmStreamClose(stream, 0);
    return r;
}

int main(void)
{
    static short pcm[SAMPLES], decoded[SAMPLES], expected[SAMPLES];
    static BYTE ima[BLOCK * BLOCKS];
    WAVEFORMATEX wfx = {WAVE_FORMAT_PCM, 1, 22050, 44100, 2, 16, 0};
    IMAADPCMWAVEFORMAT adpcm;
    DWORD drivers = 0, used = 0;
    MMRESULT r;
    int i, worst = 0;
    long total = 0;

    /* A triangle wave of 100 samples a period. */
    for (i = 0; i < SAMPLES; i++) {
        int t = i % 100;
        pcm[i] = (short)((t < 50 ? t : 100 - t) * 320 - 8000);
    }
    memset(&adpcm, 0, sizeof adpcm);
    adpcm.wfx.wFormatTag = WAVE_FORMAT_IMA_ADPCM;
    adpcm.wfx.nChannels = 1;
    adpcm.wfx.nSamplesPerSec = 22050;
    adpcm.wfx.nBlockAlign = BLOCK;
    adpcm.wfx.nAvgBytesPerSec = 22050 * BLOCK / PER_BLOCK;
    adpcm.wfx.wBitsPerSample = 4;
    adpcm.wfx.cbSize = 2;
    adpcm.wSamplesPerBlock = PER_BLOCK;

    r = acmMetrics(NULL, ACM_METRIC_COUNT_DRIVERS, &drivers);
    printf("acmMetrics: %u, %lu drivers\n", r, drivers);
    check(r == 0 && drivers >= 1, "drivers");

    r = convert(&wfx, &adpcm.wfx, pcm, sizeof pcm, ima, sizeof ima, &used);
    printf("encode: %u, %lu bytes\n", r, used);
    check(r == 0 && used == sizeof ima, "encode");

    r = convert(&adpcm.wfx, &wfx, ima, sizeof ima, decoded, sizeof decoded,
                &used);
    printf("decode: %u, %lu bytes\n", r, used);
    check(r == 0 && used == sizeof decoded, "decode");

    for (i = 0; i < BLOCKS; i++)
        decode_block(ima + i * BLOCK, expected + i * PER_BLOCK);
    check(!memcmp(decoded, expected, sizeof decoded), "decoded as IMA ADPCM");
    for (i = 0; i < SAMPLES; i++) {
        int d = decoded[i] - pcm[i];
        if (d < 0) d = -d;
        if (d > worst) worst = d;
        total += d;
    }
    /* IMA ADPCM starts each block with its smallest step, which takes a
     * few samples to catch up with a steep wave. */
    printf("error: %ld on average, %d at most\n", total / SAMPLES, worst);
    check(total / SAMPLES < 200 && worst < 4000, "close to the input");

    printf("%s\n", failures ? "FAILED" : "all ok");
    return failures != 0;
}
