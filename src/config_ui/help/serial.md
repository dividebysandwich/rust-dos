# Serial port {#com-port}

What is plugged into this serial (COM) port:

- **serial mouse**: for games and programs that want a mouse on a COM
  port. Most games use the ordinary mouse driver and don't need it.
- **modem**: plays modem games with the other player in your LAN room:
  any number the game dials calls them. It dials BBSes on the internet
  too.
- **null modem cable to LAN room**: plays serial-cable games with the
  other player in your LAN room.
- **port only, nothing plugged in**, or **off** (no port at all).

By default COM1 has a mouse and COM2 a modem. For two-player modem or
serial games, join a LAN room, then pick **COM2** in the game.

# Serial port IRQ {#com-irq}

The port's IRQ. The usual values are **4** for COM1 and COM3, **3** for
COM2 and COM4. Tell a game's setup the same, and leave them unless a
game needs otherwise.

# Serial chip (UART) {#uart}

The chip of the serial ports. **16550A** suits nearly everything; a few
very old programs want the **8250**.

# Serial mouse {#mouse-type}

The kind of serial mouse: **Microsoft** with 2 buttons, or **Logitech**
with 3 buttons for programs that use a middle button.

# Modem takes calls on {#modem-listen}

Lets another Rust-DOS, or DOSBox's modem, call your modem over the
internet on this TCP port. When it rings, the game (or `ATA`) answers.
**off** takes no calls; LAN room players can call without it.

# Modem speaks telnet {#modem-telnet}

Turn **on** for calling telnet BBSes with the modem, so their menus come
out right. Leave **off** for games played over the modem.

# Printer {#printer}

An Epson dot matrix printer on the parallel port LPT1, which programs
print to as they would to a real one. Pick the Epson driver (such as
*Epson LQ* or *Epson FX*) in the program. What it prints becomes:

- **PDF documents**: one for each print job.
- **PNG pictures**: one for each page.
- **host printer**: the job goes to your computer's printer.
- **file (as sent)**: the bytes the program sent, in a `.prn` file,
  for a printer that knows the program's own printer language.
- **none**: no printer and no LPT1.

The files go in the capture folder. **Ctrl+Shift+F5** ejects the page
and ends the job at once. A parallel port DAC takes LPT1's place.

# Paper {#printer-paper}

The paper in the printer: **Letter**, **A4** or **Legal**. Tell the
program the same.

# Resolution {#printer-dpi}

How many dots an inch the pages are drawn with. **360 dpi** is what
the printer prints at; lower makes smaller files.

# Pages of a job {#printer-multipage}

Whether a print job's pages go into **one document**, or each page into
**a document each**.

# Job ends after {#printer-timeout}

A print job ends, and its last page comes out, when the program has
printed nothing for this long. **only when ejected** waits for
**Ctrl+Shift+F5**.
