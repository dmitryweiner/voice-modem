# Protocol

The sound on the wire and the rules of the exchange, as implemented in
`crates/modem-core`. Version 0: it changes freely until the first release. Every number here has a test in the
module named next to it.

All processing runs at **16 kHz**; the core converts from and to the sample
rate of the audio device (`resample.rs`). Bits are sent most significant first;
multi-byte numbers are big-endian.

## Frame

```
[lead-in tone] [preamble: up-chirp, down-chirp] [header] [payload + CRC-32]
   optional              256 ms                  992 ms    mode R1: 64 ms per byte + 608 ms
```

A data or broadcast frame carries 0–255 bytes of payload; an acknowledgement
ends after the header. The frame fades in and out over 32 samples (`tx.rs`).

### Lead-in tone

Optional, set by the sender: a 1000 Hz tone of any length before the
preamble. It opens the VOX or the squelch of a radio and wakes up the voice
detector of a vocoder. The receiver does not need it and ignores it.

### Preamble (`sync.rs`)

Two linear chirps of 2048 samples (128 ms) each: 600 → 2600 Hz, then
2600 → 600 Hz. Each fades in and out over 32 samples.

The receiver correlates the stream with the up-chirp (as a complex
exponential, so the phase does not matter) and normalises by the energy of the
window, so the level does not matter either: the result is the share of the
window's energy that is the chirp, from 0 to 1. Above 0.012 the peak is
located to the sample, and the down-chirp is looked for after it. A frequency
offset of Δf moves the up-chirp peak Δf · 1.024 samples earlier and the
down-chirp peak as much later; the midpoint gives the start of the frame and
the distance gives Δf, which the receiver follows up to ±250 Hz (`rx.rs`).

### Header (`frame.rs`)

Seven bytes, always in mode R1:

| Byte | Content                                                                                    |
| ---- | ------------------------------------------------------------------------------------------ |
| 0    | bits 7–6: protocol version (0); bits 5–4: kind of the frame; bits 3–0: mode of the payload |
| 1–2  | station address                                                                            |
| 3    | frame number                                                                               |
| 4    | payload length in bytes                                                                    |
| 5–6  | CRC-16/CCITT-FALSE of bytes 0–4 (polynomial 0x1021, initial value 0xFFFF)                  |

| Kind | Name            | Address and number                                   | Payload                                               |
| ---- | --------------- | ---------------------------------------------------- | ----------------------------------------------------- |
| 0    | data            | of the sender; it counts its frames, wrapping around | yes; asks for an acknowledgement                      |
| 1    | acknowledgement | those of the data frame it answers                   | none: the frame ends with the header, the length is 0 |
| 2    | broadcast       | of the sender                                        | yes; expects no answer                                |

A receiver that does not know the version, the kind or the mode drops the
frame.

### Payload

The payload bytes followed by their CRC-32 (IEEE 802.3, the one of zip and
PNG), in the mode named by the header.

## Modes

| Id  | Name | Modulation              | Code                    | Net speed  |
| --- | ---- | ----------------------- | ----------------------- | ---------- |
| 0   | R1   | 16-FSK, 31.25 symbols/s | convolutional, rate 1/2 | 62.5 bit/s |

### R1 modulation (`mfsk.rs`)

One of 16 tones per symbol of 512 samples (32 ms): 750 Hz, 875 Hz, … 2625 Hz,
125 Hz apart. The phase is continuous from symbol to symbol. A symbol carries
4 bits; tone number `t` carries the value `t XOR (t >> 1)` (Gray code:
neighbouring tones differ in one bit).

The receiver measures the energy of each tone over the symbol and never needs
the phase or the level. The soft value of a bit is the amplitude of the
loudest tone that has the bit clear minus that of the loudest that has it set.

### Block coding (`coding.rs`, `conv.rs`)

The header and the payload (with its CRC) are each coded as a block on their
own:

1. **Convolutional code**, constraint length 7, rate 1/2. The register takes
   each bit into its lowest position; the two output bits are the parities of
   the register masked with 171 and 133 (octal), in this order. Six zero bits
   flush it at the end. A block of `n` bytes becomes `(8n + 6) · 2` coded bits.
2. **Interleaver**. With `L` coded bits and the stride `s`, bit `i` on the air
   is coded bit `i · s mod L`. `s` is the first number from `⌊0.618 · L⌋` up
   that has no common divisor with `L`.
3. **Scrambler**. The bits on the air are XORed with the output of a 16-bit
   Galois shift register: state 0xACE1 at the start of each block; each step
   outputs the lowest bit, shifts right and, if the bit was 1, XORs the state
   with 0xB400.
4. **Padding** with further scrambler bits up to a whole number of symbols.

The receiver undoes these on soft values and decodes with the Viterbi
algorithm.

## Link layer (`link.rs`)

Stations take turns on one channel. A station picks its 16-bit address at
random when it starts.

### Payload of data and broadcast frames

The first byte is the type in bits 3–0 and flags above: bit 6 — the first
segment of a message, bit 7 — more segments follow. An empty payload and
unknown types are acknowledged and otherwise ignored.

| Type    | Content after the first byte                                                                                       |
| ------- | ------------------------------------------------------------------------------------------------------------------ |
| 0 text  | UTF-8 text, cut into segments of up to 63 bytes anywhere, also inside a character; a message is at most 1000 bytes |
| 1 hello | capabilities, 2 bytes (none defined yet: 0), then the name of the station, up to 32 bytes of UTF-8; one segment    |

A receiver starts a message at a segment with the "first" flag, appends
segments in the order of arrival and delivers it at the one without "more".
Segments that come without a first one are dropped.

### Acknowledgement and repeats

- A station that receives a **data** frame answers with an acknowledgement
  0.5 s after the frame ended, whether or not it has seen the frame before.
  It delivers a payload only if the frame number differs from the last one
  received from that address: a repeat means the acknowledgement was lost.
- The sender waits for the acknowledgement until 0.5 s + the length of an
  acknowledgement + 1 s after its frame ended (10 s more with push-to-talk).
  Then it waits a random 0.2–1.2 s and sends the same frame again. After five
  transmissions the message has failed and its remaining segments are dropped.
- The next segment goes out only after the previous one was acknowledged
  (stop and wait).
- A **broadcast** frame is sent once; its segments follow each other 0.2 s
  apart.

### Taking turns

- A station does not listen while it transmits and for 0.3 s after (its own
  sound still comes back from the device and the room). Therefore nobody
  answers a frame sooner than 0.4 s after its end.
- A data frame is not started while a frame is being received, nor until a
  random 0.4–0.9 s after the channel fell silent, nor while an acknowledgement
  is owed. Acknowledgements ignore this: they go out on time.
- With push-to-talk nothing is transmitted until the operator says the button
  is down; everything waiting then goes in one transmission, acknowledgements
  first, with the lead-in tone once at the start.
- A greeting (hello) is answered with one's own, once per address, unless one
  was sent in the last minute.

Known limits: a data frame has no destination, so with more than two stations
every one that hears it acknowledges, at the same moment. The receiver takes
the whole frame once it has read a header and does not notice another
station's frame that starts meanwhile.

## What the receiver reports (`rx.rs`)

For every frame: the position of its first sample in the stream, the frequency
offset and an estimate of the signal-to-noise ratio, taken from the tone
energies, with the noise counted over the whole band up to 8 kHz (add 4.3 dB
for the noise in a 3 kHz band). On a clean signal the estimate tops out near
26–33 dB depending on the frame length.

A frame whose payload checksum fails, and a preamble that is unmistakable
(both chirps make up a quarter of the energy of their windows or more) but is
followed by an unreadable header, are reported as "heard but not read".

## Not there yet

Other modes, tracking of the symbol clock inside a frame, addressed
sessions, files and everything else above the link layer: see [the plan](../plans/PLAN.md).
