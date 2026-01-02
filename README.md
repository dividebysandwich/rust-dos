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

## What doesn't work

* Programs using OVLs
* sub-processes (like launching another program from within Norton Commander)
* VGA graphics
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
