# LAN rooms {#online}

Where *Find or make a LAN room* looks for rooms to play multiplayer
games in:

- **on this network**: other Rust-DOS players on your home network.
- **via Internet**: rooms on the public relay, which anyone can join,
  wherever they are.

# Relay {#relay}

The server that passes game traffic between players over the internet.
The public relay `relay.rust-dos.com` works for everyone; only change
this if you or your friends run a relay of your own.

# Find or make a LAN room {#rooms}

Multiplayer in three steps:

- Everybody opens this and joins the same room (Enter), or one player
  makes one (**Ins**) and the others join it.
- Everybody starts the game and picks its IPX, network or serial
  multiplayer option.
- For Doom and Heretic, start `IPXSETUP -nodes 2` (or the number of
  players), or `SERSETUP -com2` for two players over a serial link.

**Tab** switches between rooms on this network and online. At the prompt,
`LAN` shows the room you're in.

# LAN player name {#player}

The name the others in a room see you by. Without one, you are "Player"
and a number.

# IPX driver {#ipx}

The network driver most DOS multiplayer games use (Doom, Duke Nukem 3D,
Warcraft 2, Descent). **auto** installs it as soon as you join or host a
room, which is all most people need.

# IPX IRQ {#ipx-irq}

The IRQ the IPX driver uses. **auto** picks one that no sound card has.
Only change it if a game reports a conflict.

# IPX frame type {#ipx-frame}

How IPX packets are wrapped for the network. Only matters when talking to
Windows 95 or another system booted from a disk image with a network
card; leave it at **Ethernet II**.

# NE2000 network card {#ne2000}

A network card, for Windows 95 or 3.11 booted from a disk image, or for
DOS internet programs with a packet driver. It reaches the internet
through your computer, and the other players' cards in a LAN room.

DOS games don't need it for multiplayer: the *IPX driver* does that.

# NE2000 port {#nic-base}

Where the NE2000 sits in the PC: **300h** unless you changed it. Tell
the card's driver the same value.

# NE2000 IRQ {#nic-irq}

The NE2000's IRQ: **10** unless you changed it. Tell the card's driver
the same value. It must differ from the sound cards'.

# Ethernet address {#mac-addr}

The network card's hardware (MAC) address. **auto** makes a new one each
start, which is fine for most uses. Type a fixed one, such as
`02:00:5E:12:34:56`, if a system you installed must see the same card
each time.

# Join a LAN at startup {#lan}

Joins a room as soon as Rust-DOS starts: **discover** finds a host on your
network, or type a relay's address. **off** joins nothing. Takes effect at
the next start.

# Host a LAN at startup {#lan-host}

Hosts rooms for the others on your network from the start, on a UDP port
(**21213** is the usual one). **off** hosts nothing. Takes effect at the
next start.

# LAN room {#room}

The room *Join a LAN at startup* and *Host a LAN at startup* join.
Everyone playing together has to be in the same room.

# LAN password {#password}

The password of the room joined at startup, or the one it gets if you
make it. Empty for a room open to all.
