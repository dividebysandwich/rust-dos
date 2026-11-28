# Regression programs by area

Which programs reach which part of the emulator, for the "Regression
check" that AGENTS.md's "Checks in proportion to the change" asks of a
pull request. Find the rows the change touches, take the programs that
reach them, and check the counter or log line the row names, so the run
counts. The tables come from the "Regression check" sections of merged
and open pull requests, each cited by number: read the pull request for
the exact settings and builds.

A row lists what earlier checks found, not every program that could
reach the area. When a check finds a new program that reaches a row, or
one that looked likely and doesn't, add it here in the same pull
request.

## Fixtures

A fixture is a scene of a game that a script can replay: a plan, which
plays from power-on with input at fixed emulated times, or a save state.
Plans run deterministically, so main and a branch reach the same frames
at the same emulated times; a state is quicker, but stops loading when a
save-state version moves (the README of each fixture folder says which
of its states still load), and one saved by main can carry main's
version of what the change moves (a texture's placement, for one).

Fixtures live outside the repository, with the game's files: on the
maintainer's machine in `<Game (Year)>\fixtures\` next to the game's
`installed\` copy. In the tables, a scene is named by the game and the
file in that folder:

- `name.plan.json`: a plan. Its runner (`run.py` in the folder) starts
  the build hidden and muted with `--deterministic`, sends each event
  while paused at its emulated time, and writes the shots and
  `/api/stats` at each.
- `name@sha.state`: a state saved by build `sha` (`unknown` for states
  collected from older work, which mostly no longer load).

The scene's description (the game, its version or release, the menu
path and the times) is there so another machine can make the scene
again: install the same release, give it the same configuration, and
record a plan with the same input. "Fixture scenes" at the end lists
the settings of each scene the tables name.

Columns:

- **Reached**: the counter or log line that shows the run took the
  code. `/api/stats` and `/api/status` are the debug server's
  (`docs/llm-debugging.md`), `/api/profile`'s sections are in
  `docs/profiling.md`, the recompiler's counts in `docs/dynrec.md`
  ("Statistics"). A run that doesn't show it only shows that nothing
  else moved, and doesn't count toward a tier.
- **Det.**: whether a `--deterministic` run reaches the code. "GL env"
  means the ToadForce's OpenGL renderer, which draws in a deterministic
  run only with `RUST_DOS_TOADFORCE_GL_DETERMINISTIC=1` (check
  `renderer` in `/api/status`). The Voodoo cards' OpenGL path never
  runs in deterministic mode, and `cycles=auto` and `max` pick their
  speed from the host: those take the form for runs that can't be
  deterministic (AGENTS.md, end of "Comparing what two builds draw").
- **Doesn't reach**: programs a check ran and found outside the code,
  where they look like they would reach it.

## CPU, timing and memory

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| CPU interpreter | The game suite (`tests/program/game_suite.rs`, [game-suite.md](game-suite.md)): its `-normal` scenarios run on the interpreter with blessed hashes. Any fixture with `core=normal` | `/api/stats` `core` and `recompiler` false | yes | |
| Recompiler (dynrec), common paths | The Ultimate Doom `e1m1-start` (46 shifts by CL translated in 3 s), Quake software renderer `sw-e1m1-start` (61), both at `--cycles 60000`, 10 s (#2248). The game suite's `-dynamic` scenarios share the interpreter's goldens | `/api/stats` `dynrec` (`blocks`, `instructions`, `native`, `exits`) equal on both builds, with icount | yes | |
| Recompiler, poked code (watched bytes) | Witchaven `EGWH1.EXE` (ETTiNGRiNDER's rebuild), `04-egwh-level1`: 27 shift translations read their count from RAM, `watched` exits 143,059 on main (#2248). Delta Force (built-in Windows, Voodoo3): MMX shifts by a poked constant (#847) | `/api/stats` `dynrec.watched` | yes | Doom, Quake, Z, Bedlam poke no shift count and have no watched exits (#2248) |
| FPU (x87) | Every 3D program. Rage Rally `goldstone-race` (Voodoo3), MDK2 `demo` and Max Payne `intro-menu` (ToadForce) (#2170); Beetle Crazy Cup `urock-race` takes denormal doubles (#2170); Quake for the control word's precision (#2101) | No counter: equal icount and frames. A probe build's log line for the changed case | yes | |
| MMX, SSE, 3DNow! | Delta Force (`cpu=pentium_ii`, MMX shifts, #847); Nocturne (MMX lighting loop, #1070); Evil Islands (needs `cpu=pentium_mmx` or later). No program at hand is known to use 3DNow! (`cpu=k6_2`, #103) | `/api/profile`'s hottest guest places and `dynrec.calls.fallback_top` name the instructions | yes | |
| Translated code's writes to a frame buffer | VESA with a linear frame buffer: Quake `sw-e4m1-1024` (168 million writes in 5 s) and Terminator: SkyNET `s1-fs-start-640`, both `--cycles 150000`, 5 s (#2346). VESA through the A0000h bank window: Perfect Assassin (VESA 101h) and Ascendancy (640x480), from power-on (#2095, #1287). A Voodoo3 desktop the program locks: Delta Force from power-on to its start screen's fade, 150000 (132 million `dev` calls on main, #2274) | `/api/stats` `dynrec.calls.dev`; `/api/profile` `mapped_memory.device` (frame buffers above RAM) and `video` (A0000h to BFFFFh) | yes | Max Payne, MDK2 and Black & White on the ToadForce: `mapped_memory.device` 0, no translated access to the frame buffer (#2346) |
| Translated code's writes to a 3D card's registers | Screamer 2 (DOS, 3Dfx version, Voodoo Graphics), its `3dfx-england-mid` state, `--cycles 300000`, Up held: about 2.6 million `dev` calls a second (#2279). Outlaw Racers `menu-race` (Voodoo3 Direct3D) reaches the check and takes the old path (#2279) | `/api/stats` `dynrec.calls.dev` | yes | GLQuake through MiniGL on a Voodoo2 makes no `dev` calls (#2279) |
| `cycles=auto` pacing | Myth III 1.3.1 `yursgrad-play@3ad3af00.state` (ToadForce OpenGL) and Omikron's dialogue scene of #856 (Voodoo3, software), 4 runs a build, alternated (#2320, #2352). Nocturne, No One Lives Forever, Grom, Black & White's name entry, Quake `timedemo demo1` (#1408). Dark Omen intro and campaign movie, Prehistorik 2 intro and level 1, Johnny Castaway, G-Police briefing and flight, Iron Assault mission 2, Encarta 97's Cheetah AVI, Doom's attract demos (#1982). Real-mode loads: Alley Cat, Prince of Persia, Stunts, Commander Keen 4, Quest for Glory I, Wing Commander, Ultima VI, Lemmings, Loom, Indiana Jones 4, Civilization (#770) | `RUST_DOS_AUTO_TRACE=1` on stderr (the branch's trace shows the changed rule deciding); `/api/status` `activity` | no | |
| `cycles=max` pacing | Zanzarah `first-room-play` (Bink intro, menus, opening film, first room), Myth II 1.8.5 Willow Creek, Railroad Tycoon II's Smacker intro, Max Payne's intro and 3D menu (#2121) | `/api/profile` Win32 calls in busy time, halted share | no | |
| Idle skip | War of the Worlds (ToadForce) from power-on, Escape at 50 s, 150000; Metal Marines `02-title`, 60000; Biosys `03-menu`, 100000 (#2327). Tribal Rage `08-campaign-zone`, 112500, 30 s: count proofs following `GetTickCount` (#2253); Black & White `prologue` to 175 s, 100000 (#2253). Wizardry Gold's `PeekMessage` loop (#1711) | `/api/stats` `idle_skip` (`proofs`, `skipped`, `modelled`, `failed_by`) equal, with icount | yes | Outlaw Racers `intro-film`, MechCommander's intro and menu, Max Payne `intro-menu` and MDK2 `demo` show no count proof; only Zanzarah's proofs reach `QueryPerformanceCounter` (#2253) |
| Win32 heap | Beetle Crazy Cup `urock-race`, 150000; Demolition Racer's car select and race start, 60000; Max Payne `intro-menu` (ToadForce GL env), 150000. All three make growable heaps, 37 to 122 heap calls (#2191) | `[W32]` log: `HeapCreate` with no maximum, heap calls and addresses | yes | |
| KERNEL32's TIB: TLS and the last error | Rage Rally `goldstone-race` (84,794 `TlsGetValue` traps in the window on main), Demolition Racer `race-a68d9c59` (#2250) | `/api/profile` `hle` calls of `TlsGetValue`, `GetLastError`, `SetLastError` | yes | Beetle Crazy Cup `urock-race`: no TLS calls, one `GetLastError` (#2250) |

## Display cards

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| VGA modes and text | Mode 13h with the DOS mouse: Ultima VII Part Two (GOG, unwrapped) `si.plan.json`, 14000 cycles (#2189). EGA planar: A-Train's map (#1476). Text with a loaded font: HWiNFO for DOS (#2062). The game suite's CGA, EGA and VGA scenarios | `/api/status` `video` mode; `mapped_memory.video` | yes | |
| VESA modes | Linear frame buffer: Quake `sw-e4m1-1024` and `sw-e2m1-800`, Terminator: SkyNET `s1-fs-start-640` (#2346). Bank window: Ascendancy, Perfect Assassin (#2095, #1287); Z in 640x400 | `/api/status` `video`; `mapped_memory.device` or `video` | yes | |
| Voodoo Graphics and Voodoo2, software rasterizer | Redguard (Glide 2 under DOS/4GW, `RGFX.EXE`, `voodoo=true`, so a Voodoo Graphics with two texture units) `ship-fight.plan.json`, 300000 (#2294, #2347). GLQuake through 3dfx's MiniGL on a Voodoo2, attract demo from power-on, 150000 (#2356). Screamer 2 on a Voodoo Graphics (#2279) | `/api/status` `video.voodoo.triangles`; a probe build's counts of the variant or setup changed | yes | |
| Voodoo texture setup (multibase, trilinear) | Leadfoot `glide-race` (the game it was made for, #2294) | probe build's counts of multibase and trilinear setups | yes | Redguard `ship-fight`, Rage Rally `goldstone-race`, Demolition Racer `race-a68d9c59` set neither (#2294) |
| Voodoo draw variants (one or two texture units) | Redguard `ship-fight` (2.6 million two-unit triangles whose unit 0 ignores unit 1), Leadfoot `glide-race.plan.json` to 168.65 s, 150000 (7 million two-unit triangles that use both) (#2347). Beetle Crazy Cup `urock-race` shares Rage Rally's colour path (#2356) | probe build's counts of triangles by variant | yes | |
| Voodoo2 2D blitter | Prince of Persia 3D, Direct3D through 3dfx's Voodoo2 driver, `svga_s3virge` with a Voodoo2, to the menu at 128.5 s: 5 screen-to-screen copies (#2122) | a log line in `blt_mirror_rect` or at every blitter command (probe build) | the blitter yes; the OpenGL recording no | Test Drive 4 (Glide), Incoming: no screen-to-screen copies or fills. Moto Racer (GOG, Glide): SGRAM fills only. Lands of Lore 2's Glide build draws in software (#2122) |
| Voodoo OpenGL renderer | Rage of Mages (unwrapped) `m1-river`, `voodoo_renderer=opengl`, `--hidden`, 450000 (#2274, #2255); Biosys `05-stream` (#2255) | `/api/status` `video.voodoo.opengl`, `gpu_2d` (`direct_grants`, `pages_read_back`), `gpu_read_backs` | no | CyberStorm 2 `08-mission1-start`: no accesses to the card's memory (#2255) |
| Banshee and Voodoo3 2D engine | Glover on a Windows 98 SE image with 3dfx's Voodoo3 driver 4.11.01.1151, 50000; Moto Racer on a Windows 95 OSR2.5 image with the Voodoo3 driver, 60000 (#2268). The copies are the desktop's and the launch's | `/api/status` `video.voodoo.banshee.plain_copy_pixels` | yes | |
| Banshee and Voodoo3 scanout at 24 and 32 bits | Windows 98 SE with 3dfx's Voodoo3 driver 4.11.01.1151 at 640x480 True Color: `vidProcCfg` `080C0081` (#2231). Max Payne sets a 32-bit mode with a gamma ramp | `/api/status` `video.name`, the Banshee's `vidProcCfg` | yes | Every Voodoo3 game fixture runs on a 16-bit desktop: Leadfoot `d3d-race`, Rage Rally, Beetle Crazy Cup, Demolition Racer, MDK2, Black & White (#2231) |
| Voodoo3 Direct3D HAL (the built-in Windows) | Beetle Crazy Cup `urock-race`, Rage Rally `goldstone-race`, Demolition Racer `race-a68d9c59`, Leadfoot `d3d-race`, Outlaw Racers `menu-race` (all `svga_voodoo3`, built-in Windows 98 SE). DirectDraw flips on the software Voodoo3: Unreal's D3D flyby, G-Police mission 1, Dark Omen's first battle (#2067) | `/api/status` `video.voodoo.triangles`; `--trace-3d` counts of render states | yes | Only Outlaw Racers draws with SRCBLEND of SRCCOLOR or INVSRCCOLOR, of 148 `--trace-3d` count files (#2212) |
| PowerVR PCX1 and PCX2 | Tomb Raider's PCX1 build (`TOMBPCX1.EXE`, `powervr=pcx1`) and Unfinished Business for the PCX2 (`TUBPCX2.EXE`), the same plan to 80 s (#2128) | `/api/status` `video.powervr.renders` | yes | |

## ToadForce

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| Direct3D through the ToadForce, software renderer | Max Payne `intro-menu`, MDK2 `demo`, Nocturne `volume1-hq`, Black & White `prologue`, all from power-on (#2259, #2247) | `/api/status` `video.toadforce.commands` (`draws`, `transformed`, `primitives`) and `renderer` `software` | yes | |
| The ToadForce's OpenGL renderer, common path | Max Payne `intro-menu`, 150000 (25 shots to 70 s); MDK2 `demo`, 100000 (to 185 s); Nocturne `volume1-hq`, 150000 (to 215 s); Black & White `prologue`, 150000; Myth II `willow-creek-play` state and Zanzarah `first-room-play` state, 112500 (#2259, #2247, #2346) | `/api/status` `video.toadforce.renderer` `OpenGL`, `host.counts` (`batches`, `runs`, `transformed_runs`, `flushes`), `host.read_backs` | GL env | |
| ToadForce: draws the card transforms with its own depth | Zanzarah `first-room-play`: 720 biased draws (#2247) | `host.counts` (the card's depth runs) | GL env | Nocturne, Myth II, Black & White, Max Payne, MDK2 have no such draws (#2247) |
| ToadForce depth fills | Nocturne `volume1-hq` (DDBLT_DEPTHFILL, 12 fills by 215 s, #2066) | `video.toadforce.commands.fill` | GL env | |
| OPENGL32 | On the ToadForce: Soldier of Fortune (Platinum 1.07f) and Soldier of Fortune II (Gold 1.03), GOG, unwrapped (SoF2's `opening-cutscenes.plan.json`; texture names, #1852). On a Voodoo2: GLQuake with the built-in OPENGL32 (#1527) | `/api/win32/ddraw` `gl_draws` (`card`, `refused`, and the processor's by reason) | GL env | |

## DirectDraw and Direct3D

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| CreateSurface and GetCaps | Beasts and Bumpkins (8-bit, flip chain, 367 CreateSurface), The Tone Rebellion, Biosys, `svga_s3`, `win32_depth=8`, 200000 (#2134). Nocturne `volume1-hq` (13 CreateSurface), Black & White `prologue` (82) (#2152) | `--trace-3d` and `[W32]` counts of `CreateSurface` | yes | |
| SetSurfaceDesc and Flip | Myth II 1.3 with Project Magma 1.8.5, `willow-creek-play.plan.json`, ToadForce GL env, 112500: 5840 `SetSurfaceDesc`, 739 `Flip` (#2206) | `[W32]` trace of `Surface3_SetSurfaceDesc` and `Surface_Flip` | yes | Myth II's textures are single-level, so it doesn't reach the complex-surface case (#2206) |
| Blits with a clipper | Stratosphere: Conquest of the Skies `build-mode` and `flying` states, 200000: 1260 and 550 clipped blits (#2126) | `/api/win32/ddraw` (a surface with a clipper); `Surface_Blt` in the log | yes | Most of 47 built-in-Windows fixtures draw by locking or `BltFast` (#2126) |
| DXT surfaces | No One Lives Forever to its main menu, 200000: character textures decoded through the shared path, 397 Blt and BltFast (#2154). Rage Rally (compression into DXT, #2154) | `[W32]` Blt and BltFast counts | yes | Evil Islands and Black & White load DXT textures with `IDirect3DTexture::Load` (#2154) |
| A windowed primary's palette through GDI | Bedlam's menu (#2118) | `[W32]` `SetPalette` on the primary | yes (#2118's runs used wall-clock waits) | |

## GDI and USER (Win32)

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| Palettes | Memphis Math (title, log-in, Objective Selection, path screen), `win32_depth=8`, 100000; Metal Marines (intro and command info), 100000, `cpu=pentium`: 175 and 301 foreground `RealizePalette` calls (#2139). This Means War! (#2045, #2024) | `[W32]`/`[W16]` `SelectPalette(..., 0)` then `RealizePalette` | yes | |
| Regions | Metal Marines (title, island editor, 13,000 combines), Memphis Math's `INSTALL.EXE` dialog (450), Chaos Overlords power-on to the title (130) (#2284) | probe build's count of combines | yes | |
| Bitmaps and memory DCs | Civilization II (`CIV2.EXE` selects the stock bitmap back about 200 times a session, #2336). Max Payne `intro-menu`, MDK2 `demo` (#2169) | `[W32]` `SelectObject`, `DeleteObject` | yes | Only Beetle Crazy Cup's launcher passes a DC to `DeleteObject`, of 619 programs' logs (#2169) |
| Bitmap fonts and the System font | Castle of the Winds 1, Kye, Klotz, Jewels, Missile Attack, Mah Jongg for Windows (Win16 shareware), 20000, shots at 6 and 10 s (#2282). Outlaw Racers' installer `SOLDSET.EXE` (IRSETUP's Welcome page) for `realized_of` (#2208) | the text drawn in the System font differs only where expected | yes | SkiFree draws in another font (#2282) |
| TrueType text | CyberStorm 2 `04-prefs` state, then the Terrain tab: labels in Arial (#2287) | the labels' pixels | yes | Castle of the Winds 1, Squirmer, Mah Jongg's VIEWSET, Beetle Crazy Cup to car select, TILES: no TrueType face in 30 s (#2287). Magic and Mayhem: The Art of Magic's InstallShield setup draws with bitmap fonts on a fresh C: (#2303) |
| USER messages, painting, clipping | Max Payne's InstallShield 6 setup, welcome and licence pages (transparent statics, #2217). The Art of Magic's InstallShield 6 setup, 100000 (#2295, a run that shows nothing else moved). Chaos Overlords 1.1 (GOG, unwrapped) `map-turn1`, menu tracking (#2229) | a probe build's log line where the order changes; `[W32]` `WM_PAINT` | yes | No fixture program creates a `WS_EX_TRANSPARENT` window but those setups (#2295) |
| Focus and activation | MechCommander's US CD setup (18 `SetForegroundWindow`), Delta Force's US CD autorun, DirectX and InstallShield setups (#2162) | `[W32]` `SetForegroundWindow` and `ActivateInQueue` flags | yes | |
| Resources | Stratosphere's CD setup (InstallShield 5), The Art of Magic's CD setup (`FindResourceExA` for language 0009h), Stratosphere installed to its menu; 150000, `svga_voodoo3` (#2160) | `[W32]` counts of `FindResourceA`, `LoadStringA`, `LoadBitmapA` | yes | No program at hand calls `SendMessageTimeoutA` (#2160) |
| The desktop's depth (`win32_depth`) | Leadfoot `d3d-race` to 73.7 s, Memphis Math's `INSTALL.EXE`, Max Payne windowed (`-window`) (#2318) | `/api/status` `video` mode | yes | |
| COMCTL32 wizards, edit controls | No other program found: setups' paged licence boxes would reach Page Down (#2241); MFC's AddPage sends `PSM_ADDPAGE` only once the sheet exists (#2210) | | | |

## Win16

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| KERNEL selectors and tasks | SimTower demo's `SETUP.EXE` (Microsoft Setup bootstrapper): 31 picks differ from first fit (#2283). Castle of the Winds 1 for USER's DGROUP, 100000 (#2349) | probe build's log line in the changed pick | yes | Castle of the Winds 1, Mah Jongg, Kye, Klotz, Jewels free nothing then allocate (#2283) |
| USER and GDI (16-bit) | Memphis Math, Metal Marines (palettes, #2139; regions, #2284; idle skip, #2327). Castle of the Winds 1 and the shareware set (fonts, #2282). Metal Marines' CD launcher and setup (`ExitWindows`, #2239) | `[W16]` log lines | yes | Only the WinG SDK's setup calls `ExitWindows`, and only on Windows 3.x (#2239) |
| TOOLHELP | Outpost 1.51's setup (`NotifyRegister`, #2233) | `[W16]` `NotifyRegister` | yes | WinRoids, the Lemmings for Windows demo setup, the WinG SDK's WINGBUG and _MSTEST call only `TimerCount`, `MemManInfo`, `SystemHeapInfo` (#2233) |
| MMSYSTEM | Battle Beast (MIDI output, `timeSetEvent`, waveIn, #1780). The Magic School Bus Explores the Solar System (mmio procedures, #2319; MCI `setvideo`, #2332). Civilization II (waveOut, Indeo 4 films, #2339, #2357). Memphis Math's opening (AAPLAY.DLL draws `OPENING.FLC` itself), `play-and-exit`, 20000 (#2226) | `[W16]` mmio and MCI calls | yes | No other 16-bit program at hand calls `mmioOpen` (#2319) |
| COMPOBJ, OLE 2, STORAGE | Civilization II's 16-bit AVIFILE, which opens AVI files through a class it registers (#2302); The Magic School Bus (STORAGE, #2308) | `[W16]` `CoCreateInstance`, `StgOpenStorage` | yes | No other 16-bit program at hand asks COMPOBJ for a registered class (#2302) |

## Sound

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| Sound Blaster, OPL, GUS, General MIDI (DOS) | The game suite: SB 2.0, SB Pro 2, SB16, Gravis Ultrasound, General MIDI, the PC speaker and the Disney Sound Source, with blessed sound hashes. The Ultimate Doom (`ultdoom-game-*.conf`: SB16 with OPL and General MIDI, GUS, SB Pro 2, SB 2.0, AdLib, PC speaker; `title-sb`, `gus-e1m1` states). Prehistorik 2 `title-music-start.plan.json` (SB DMA, #424). Covert Action and Utopia (OPL, #1694, #1558, `--cycles 3000`) | `/api/status` `audio.peak`; a capture compared between builds; `/api/profile` zones `sb.dma`, `gus.advance`, `audio.synth` | yes | |
| DirectSound | Omikron's name entry, 200000 (#1225). Rage Rally (DirectSound) | an audio capture of both builds | yes | |
| waveOut and MIDI (16-bit) | Battle Beast (MIDI output and waveIn, #1780). Civilization II (waveOut, #2339) | `[W32]` `waveOut*` and `mci*` calls | yes | |
| ACM | Demolition Racer's WMA music through `msaud32.acm` on `installed-wma`, with Windows 98 SE's seven stock codecs listed before it (#2178) | `[W32]` `acmFormatSuggest`, `OpenDriver`; `audio.peak` | yes | No program that works on main gets past `acmFormatSuggest` to a driver for a format the built-in codecs don't take (#2178) |

## Video playback

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| Video for Windows: Microsoft Video 1 | Biosys's intro film from power-on, 200000, 24 `ICDecompress` calls (`vidc`/`MSVC`) (#2200) | `[W32]` `ICLocate`, `ICDecompress` | yes | |
| Video for Windows: Cinepak and Indeo | Outlaw Racers `intro-film.plan.json` (Cinepak through `AVIStreamRead` and `ICDecompress`, #2200). Leadfoot's Indeo 5 intro films (`ICDecompressEx`, #2220). Civilization II's `OPENING.AVI` and `LOSER.AVI` (Indeo 4 at 8 bits, #2357). Wizardry Gold (Cinepak) | `[W32]` `ICDecompress`, `ICDecompressEx`; the films' frames | yes | No other installed game has Indeo 4 films (#2357). No other program asks `ICLocate`/`ICOpen` for Cinepak or Indeo (#2200) |
| DirectShow | Magic & Mayhem's movies (QUARTZ, AMSTREAM, Indeo 4, #531); Submarine Titans' movies (#755, #761); Beetle Crazy Cup's AMSTREAM movies; the Indeo 5 codec's `DllRegisterServer` (`IFilterMapper2`, #2188) | `[W32]` QUARTZ and AMSTREAM calls | yes | |
| MCI devices | Memphis Math's opening, Outpost's FLC films through an installable driver (#2226). The Magic School Bus (16-bit MCI, #2332) | `[W16]`/`[W32]` `mciSendCommand` | yes | |
| Smacker and Bink | Railroad Tycoon II's Smacker intro (through GDI and DirectDraw), 60000 (#2121). Zanzarah's and Demolition Racer's Bink movies. Witchaven's Smacker intro (`01-intro-smacker`) | the films' frames | yes | |

## Input, networking, files, registry, COM

| Area | Programs and scenes | Reached | Det. | Doesn't reach |
|---|---|---|---|---|
| DirectInput | Beetle Crazy Cup `urock-race`: 2 `DirectInputCreateA`, 2 `CreateDevice`, 225 `GetDeviceState` (#2273). Rage Rally's relative mouse | `[W32]` DirectInput calls | yes | |
| DirectPlay | Sorry!'s LAN host (#2267); Zax's multi-player menu (DirectPlay 8, #1574); Magic & Mayhem: The Art of Magic (DPNET, #1247); Drakan's DirectPlay 6.1a package (#2157) | `[W32]` `DirectPlayEnumerate`, `EnumConnections` | yes | Bedlam imports DPLAY.DLL but its recorded sessions never list providers (#2267) |
| DOS and Win32 file calls, overlay drives | Beetle Crazy Cup `urock-race`, 150000 (`CreateFileA`, `GetFileSize`, `ReadFile`, a `WriteFile` into the overlay); Knights of Xentar `title-newgame.plan.json`, 20000 (DOS opens, found and missing) (#2258) | `[DOS] Open File` and `[W32]` lines in the log | yes | No other program renames onto a shown system file (#2184) |
| Registry | The DirectX 3 and 5 setups of Amok's CD menu and Beasts and Bumpkins' SETUP.EXE (#2245); Rage Rally imports `RAGERALY.REG` with REGEDIT | `[W32]` `Reg*` calls | yes | |
| COM and OLE (Win32) | The Art of Magic's CD setup (InstallShield, a local server): 2212 OLE32 calls, 86 through proxies; Black & White `prologue`: 25 in-process (#2176). Outlaw Racers' Visual Basic 5 front end (OLEAUT32, #2208) | `[W32]` OLE32 call counts | yes | |
| Setups (SETUPX, ADVPACK, InstallShield) | Amok's "Install Direct X" (DirectX 3), Drakan's InstallShield with DirectPlay 6.1a, Sub Culture's DXSETUP (#2157). MechCommander's and Delta Force's US CD setups (#2162) | `[W32]` SETUPX calls; the setup's end | yes | |

## Tiers

The tiers are AGENTS.md's. For each, the rows above that a change in
that tier usually touches.

- **Tier 1** (two to four other programs): "CPU, timing and memory"
  (all of it), "Display cards" and "ToadForce" (a card's setup, its
  common draw path, its renderer), the Sound Blaster, OPL and GUS rows
  (the game suite covers them), and "DOS and Win32 file calls". A
  change to an OpenGL path or to `cycles=auto` or `max` takes the
  non-deterministic form: pick from the rows whose Det. column says so,
  and time the programs the row names on a quiet machine.
- **Tier 2** (one other program that reaches the path): "DirectDraw and
  Direct3D", "GDI and USER (Win32)", "Win16", the Win32 and 16-bit
  rows of "Sound" and "Video playback", and DirectInput, DirectPlay,
  the registry and COM. The rows' "Doesn't reach" columns record the
  searches that found no program, which saves the next agent the same
  search; still check the counter on the program you pick.
- **Tier 3** (no other program): a row is only relevant here when part
  of the change also touches a path working programs take; that part
  is tier 2 and picks from the row as above.

## Fixture scenes

The settings of the scenes named above. The game's own setup is what
the fixture README describes; "built-in 98 SE" is `win32=true`,
`win32_version=98se`. Runs start at the plan's start time.

| Game | Release and install | Machine | Scene | Cycles, start time |
|---|---|---|---|---|
| Beetle Crazy Cup (2000) | CD, installed with its SETUP.EXE, VideoSetup 640x480x16 | built-in 98 SE, `svga_voodoo3`, `pentium_ii`, 128 MB, C: under an overlay | `urock-race.plan.json`: Escape through the intro, Game, Quick Race, the race at U-Rock from 84 s, shots to 90 s | 150000, 2000-06-01 12:00:00 |
| Black & White (2001) | installed | ToadForce (`pr2066-toadforce.conf`) | `prologue.plan.json`: Escape through the intro, name `abc` at 90 s, the in-engine prologue from 150 s | 150000, 1999-06-01 12:00:00 |
| Biosys (1999) | installed | built-in 98 SE, `svga_voodoo3` | states `03-menu`, `05-stream` | 100000 to 200000 |
| Chaos Overlords (1996) | 1.1, GOG, unwrapped | Voodoo3 | states `map-turn1`, `title` | 200000 |
| CyberStorm 2 (1998) | installed | built-in 98 SE, `svga_voodoo3`, `pentium_ii`, 64 MB | states `04-prefs`, `08-mission1-start` | |
| Demolition Racer (1999) | European CD, `installed-wma` (WMA runtime copied in, 800x600, fog, one player) | built-in 98 SE, `svga_voodoo3`, `pentium_ii`, 128 MB | `select-car-race.plan.json` (to Select Car on main from a68d9c59), `race-a68d9c59.plan.json` (the race from 135 s) | 60000, 1999-11-01 12:00:00 |
| Knights of Xentar (1994) | DOS | real mode, C: under an overlay | `title-newgame.plan.json`: intro, title menu, new game, shots 30 to 135 s | 20000 |
| Leadfoot (2001) | CD, setup Full | built-in 98 SE, `svga_voodoo3`, `pentium_ii`, 128 MB | `d3d-race.plan.json`, `glide-menus.plan.json`, `glide-race.plan.json` (the Glide driver, to the race at 168.65 s; for builds that play the Indeo 5 intro) | 150000, 2001-06-01 12:00:00 |
| Max Payne (2001) | installed, the disc's no-CD `MaxPayne.exe` | ToadForce, OpenGL (`pr2066-toadforce.conf`) | `intro-menu.plan.json`: launcher Play at 5 s, the in-engine intro, the 3D menu | 150000, 1999-06-01 12:00:00 |
| MDK2 (2000) | installed | ToadForce, OpenGL | `demo.plan.json`: launcher, Enter twice, Escape through the intro, the 3D demo from 125 s | 100000, 1999-06-01 12:00:00 |
| Memphis Math (1994) | CD | built-in 98 SE, `win32_depth=8` | `login-objective.plan.json`, `install-dialog.plan.json`, `play-and-exit.steps.json` | 20000 to 100000, 1995-04-11 12:34:56 |
| Metal Marines (1995) | CD, Win16 | built-in 98 SE, S3, `win32_depth=8`, `cpu=pentium` | state `02-title` | 30000 to 100000 |
| Myth II (2000) | 1.3 with Project Magma 1.8.5 (`installed-patched-1.8.5`) | ToadForce, OpenGL (`myth2-185.conf`) | `willow-creek-play.plan.json`: Willow Creek on Normal, 15 s into the mission; state `willow-creek-play@3ad3af00` with its `.overlay` | 112500, 1995-04-11 12:34:56 |
| Myth III (2001) | 1.3.1 | ToadForce, OpenGL (`myth3-131.conf`) | `yursgrad-play.plan.json`: Defense of Yursgrad, 50 s in; state `yursgrad-play@3ad3af00` | 112500, 1995-04-11 12:34:56 |
| Nocturne (1999) | installed | ToadForce, OpenGL | `volume1-hq.plan.json`: Escape at 35 s, Start, Volume 1, the HQ in 3D from 105 s | 150000, 1999-06-01 12:00:00 |
| Outlaw Racers (1998) | Sold Out Software CD of 1999, `SOLDSET.EXE`, `config.dat` set to Hardware Acceleration | built-in 98 SE, Voodoo3, `pentium_ii`, 128 MB | `intro-film.plan.json`, `menu-race.plan.json`, `rain-race.plan.json` | 150000, 1999-06-01 12:00:00 |
| Prehistorik 2 (1993) | DOS | SB | `title-music-start.plan.json`, `level1-start-det10000.plan.json` | 10000 |
| Quake (1996) | GOG, 1.08 | DOS, `cpu=pentium`, 32 MB, SB16 (`quake-sw.conf`) | states `sw-e1m1-start`, `sw-e2m1-800`, `sw-e4m1-1024` | 60000 or 150000 |
| Rage Rally (2000) | 2002 Lithuanian CD, laid out from the MSI's tables, `RAGERALY.REG` | built-in 98 SE, `svga_voodoo3`, `pentium_ii`, 128 MB | `goldstone-race.plan.json`: Time Trial, Goldstone, 10 s holding Up, shots to 70 s | 150000, 2000-06-01 12:00:00 |
| Rage of Mages (1998) | unwrapped | built-in 98 SE, `svga_voodoo3`, `pentium_ii` | state `m1-river` | 450000 |
| Redguard (1998) | USA discs, the 3Dfx version (`RGFX.EXE`) | built-in 98 SE, `voodoo=true`, `cpu=pentium`, 32 MB | `ship-fight.plan.json` (state at 80 s), `harbour.plan.json` | 300000 |
| Stratosphere (1998) | installed | built-in 98 SE, `svga_voodoo3`, `pentium_ii`, 64 MB | states `build-mode`, `flying` | 200000 |
| Terminator: SkyNET (1996) | DOS | 32 MB, AWE32; VESA 640x480 | state `s1-fs-start-640` | 150000 |
| The Ultimate Doom (1995) | GOG | DOS, `ultdoom-game*.conf` for each sound card | states `e1m1-start`, `title-sb`, `gus-e1m1` | 60000 |
| Tribal Rage (1998) | installed | built-in 98 SE, ToadForce, `pentium_ii`, 128 MB | state `08-campaign-zone` | 112500 |
| Ultima VII Part Two (1993) | GOG, unwrapped | DOS, mode 13h | `si.plan.json` | 14000 |
| War of the Worlds (1998) | installed | built-in 98 SE, ToadForce, `pentium_ii`, 128 MB | from power-on: intro film, Escape at 50 s | 150000 |
| Witchaven (1995) | `EGWH1.EXE`, ETTiNGRiNDER's rebuild of the engine | DOS | state `04-egwh-level1` | 60000 |
| Zanzarah (2002) | English 1.010 (`zanzarah-en.conf`) | ToadForce | `first-room-play.plan.json`: a new game, the first room 25 s after the opening film; state `first-room-play@3ad3af00` with its `.overlay` | 112500, 1995-04-11 12:34:56 |
