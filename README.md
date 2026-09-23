# Rust-DOS

<img width="640" height="400" alt="image" src="https://img.playspoon.com/t9vbqf.gif" />

## Introduction

Rust-DOS is a DOS emulator. It is a work in progress and most programs don't run yet. It does however contain a good amount of CPU mnemonics and interrupts implemented, and can run simple programs.

*Rust-DOS is looking for contributors!*

## Why

I wanted to learn more about the nuances of DOS emulation. Also, there's only one other DOS emulator written in Rust, and that one hasn't seen any development in 5 years and was using lots of unsafe{} code blocks.

## What works

* Executing COM and EXE programs
* Basic disk operations
* Passthrough filesystem
* CGA graphics
* FPU emulation
* Interrupt handlers

## What partially works

* AdLib sound
* VGA graphics
* Programs using OVLs
* TSRs

## What's not implemented yet

* Mounting additional drives
* Mounting disk images
* XMS/EMS
* IRQs and DMA
* Sound Blaster
* Gravis Ultrasound
* 640x480x16
* VESA modes

## Keyboard shortcuts

F12: Debug mode
PrintScreen: Toggle screen recording to a video file

## Debug & remote-control server

Start the emulator with `--debug-server` to expose a local HTTP/WebSocket
interface (default `127.0.0.1:8086`, or `--debug-server 127.0.0.1:9000`). It is
meant for scripted and LLM-driven debugging: taking screenshots, reading the
text screen, typing input, tracing instructions, setting breakpoints, and
inspecting registers and memory. It has no authentication and no encryption,
so keep it bound to localhost.

`GET /` lists every endpoint. Some examples:

```sh
curl localhost:8086/api/status
curl 'localhost:8086/api/screen/text?format=text'
curl -o screen.png localhost:8086/api/screenshot
curl -XPOST -H content-type:application/json -d '{"text":"dir\n"}' localhost:8086/api/input/type
curl -XPOST -H content-type:application/json -d '{"enabled":true}' localhost:8086/api/trace
curl 'localhost:8086/api/trace?last_ms=200&no_bios=true'
curl -XPOST -H content-type:application/json -d '{"addr":"1234:0100"}' localhost:8086/api/breakpoints
curl 'localhost:8086/api/control/wait?timeout_ms=10000'
curl 'localhost:8086/api/memory?addr=DS:SI&len=64'
curl -XPUT -H content-type:application/json -d '{"path":"/path/to/game"}' localhost:8086/api/drive/C
```

WebSocket streams:

* `/ws/events`: log lines, pause/resume, and video mode changes.
* `/ws/trace`: live instruction trace batches.
* `/ws/screen?fps=5`: PNG frames, sent only when the screen changes.
* `/ws/audio`: s16le 44.1 kHz mono PCM.
* `/ws/input`: input events.

To run without a window or sound device (for example under an agent), set
`SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy`. The instruction trace ring holds
`--trace-capacity` entries (default 1,000,000, about 64 bytes each). It is
allocated only once tracing is enabled.
