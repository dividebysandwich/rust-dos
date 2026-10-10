// Nuked-SC55 (jcmoyer's fork, at the commit crates/sc55 was ported from)
// switched on with given ROMs and MIDI bytes posted at given frames, for
// tests/sc55_tests.rs to compare with the port: writes every frame at the
// chip's rate to stdout as two little-endian int32s.
//
// usage: harness <family> <frames> <events> rom1=path rom2=path ...
// <events> has a line per message: the frame, then the bytes in hex.
// With DUMP_FILE, DUMP_FROM, DUMP_TO (and DUMP_EVERY) it writes the
// registers and PCM memory, as Sc55::debug_state has them, for those
// frames.
//
// Build it against the fork's legacy interpreter, which the port follows
// (decoder2 doesn't raise the H8's exceptions):
//
//   git clone https://github.com/jcmoyer/Nuked-SC55 target/sc55diff/fork
//   git -C target/sc55diff/fork checkout 02f6e3d7bad89af33514bd48211bb950f8ad0e6b
//   cmake -S target/sc55diff/fork -B target/sc55diff/build -DCMAKE_BUILD_TYPE=Release -DNUKED_ENABLE_DECODER2=OFF
//   cmake --build target/sc55diff/build --target nuked-sc55-backend
//   g++ -std=c++20 -O2 -Itarget/sc55diff/fork/src/backend -Itarget/sc55diff/build/backend \
//       tests/sc55diff/harness.cpp target/sc55diff/build/libnuked-sc55-backend.a -o target/sc55diff/harness
#include "emu.h"
#include "mcu.h"
#include "pcm.h"
#include "rom_io.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <sstream>
#include <string>
#include <vector>

struct Out { std::vector<AudioFrame<int32_t>> frames; };

static void cb(void* user, const AudioFrame<int32_t>& f) { ((Out*)user)->frames.push_back(f); }

int main(int argc, char** argv) {
    if (argc < 5) { fprintf(stderr, "usage\n"); return 2; }
    Romset romset;
    if (!ParseRomsetName(argv[1], romset)) { fprintf(stderr, "romset?\n"); return 2; }
    long frames = atol(argv[2]);
    std::multimap<long, std::vector<uint8_t>> events;
    {
        std::ifstream in(argv[3]);
        std::string line;
        while (std::getline(in, line)) {
            std::istringstream ls(line);
            long at; ls >> at;
            std::vector<uint8_t> bytes; std::string hex;
            while (ls >> hex) bytes.push_back((uint8_t)strtol(hex.c_str(), nullptr, 16));
            events.emplace(at, bytes);
        }
    }
    RomsetInfo info;
    std::map<std::string, RomLocation> names = {{"rom1", RomLocation::ROM1}, {"rom2", RomLocation::ROM2},
        {"smrom", RomLocation::SMROM}, {"wave1", RomLocation::WAVEROM1}, {"wave2", RomLocation::WAVEROM2},
        {"wave3", RomLocation::WAVEROM3}};
    for (int i = 4; i < argc; i++) {
        const char* eq = strchr(argv[i], '=');
        info.rom_paths[(size_t)names.at(std::string(argv[i], (size_t)(eq - argv[i])))] = eq + 1;
    }
    if (!LoadRomset(info, nullptr)) { fprintf(stderr, "load failed\n"); return 1; }
    Emulator emu;
    EMU_Options opts{};
    emu.Init(opts);
    if (!emu.LoadRoms(romset, info)) { fprintf(stderr, "loadroms failed\n"); return 1; }
    emu.Reset();
    Out out;
    emu.SetSampleCallback(cb, &out);
    long produced = 0;
    size_t taken = 0;
    auto it = events.begin();
    std::vector<int32_t> buf;
    long dump_from = getenv("DUMP_FROM") ? atol(getenv("DUMP_FROM")) : -1;
    long dump_to = getenv("DUMP_TO") ? atol(getenv("DUMP_TO")) : -1;
    FILE* dump = getenv("DUMP_FILE") ? fopen(getenv("DUMP_FILE"), "w") : nullptr;
    while (produced < frames) {
        if (taken >= out.frames.size()) {
            out.frames.clear(); taken = 0;
            // Post everything due at or before this frame, as the Rust side
            // does when its frames run out.
            while (it != events.end() && it->first <= produced) { emu.PostMIDI(std::span<const uint8_t>(it->second)); ++it; }
            while (out.frames.empty()) {
                emu.Step();
                if (dump && getenv("TRACE") && produced >= dump_from && produced < dump_to) {
                    mcu_t& m = emu.GetMCU();
                    fprintf(dump, "%ld %02x:%04x sr=%04x r=%04x %04x %04x %04x %04x %04x %04x %04x ip=%llx sl=%d\n", produced, m.cp, m.pc, m.sr, m.r[0], m.r[1], m.r[2], m.r[3], m.r[4], m.r[5], m.r[6], m.r[7], (unsigned long long)*(uint64_t*)&m.interrupt_pending, m.sleep);
                }
            }
        }
        if (dump && produced >= dump_from && produced < dump_to && produced % (getenv("DUMP_EVERY") ? atol(getenv("DUMP_EVERY")) : 1) == 0) {
            mcu_t& m = emu.GetMCU(); pcm_t& p = emu.GetPCM();
            fprintf(dump, "%ld", produced);
            for (int i = 0; i < 8; i++) fprintf(dump, " %d", m.r[i]);
            fprintf(dump, " %d %d %d %llu", m.pc, m.cp, m.sr, (unsigned long long)m.cycles);
            for (int i = 0; i < 32; i++) for (int j = 0; j < 8; j++) fprintf(dump, " %u", p.ram1[i][j]);
            for (int i = 0; i < 32; i++) for (int j = 0; j < 16; j++) fprintf(dump, " %u", p.ram2[i][j]);
            fprintf(dump, " %d %d %d %d %u %u %u", p.accum_l, p.accum_r, p.rcsum[0], p.rcsum[1], p.tv_counter, p.voice_mask, p.voice_mask_pending);
            long long e = 0; for (int i = 0; i < 0x4000; i++) e += (long long)(i + 1) * p.eram[i];
            fprintf(dump, " %lld\n", e);
        }
        buf.push_back(out.frames[taken].left); buf.push_back(out.frames[taken].right);
        taken++; produced++;
        if (buf.size() >= 65536) { fwrite(buf.data(), 4, buf.size(), stdout); buf.clear(); }
    }
    fwrite(buf.data(), 4, buf.size(), stdout);
    return 0;
}
