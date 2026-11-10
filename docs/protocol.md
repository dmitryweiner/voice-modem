# Protocol: physical layer

The sound on the wire, as implemented in `crates/modem-core`. Version 0:
it changes freely until the first release. Every number here has a test in the
module named next to it.

All processing runs at **16 kHz**. Bits are sent most significant first;
multi-byte numbers are big-endian.

## Frame

```
[lead-in tone] [preamble: up-chirp, down-chirp] [header] [payload + CRC-32]
   optional              256 ms                  736 ms      mode R1: 64 ms per byte + 608 ms
```

One frame carries 0–255 bytes of payload. The frame fades in and out over
32 samples (`tx.rs`).

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

Five bytes, always in mode R1:

| Byte | Content                                                                   |
| ---- | ------------------------------------------------------------------------- |
| 0    | high half: protocol version (0); low half: mode of the payload            |
| 1    | frame number, wraps around                                                |
| 2    | payload length in bytes                                                   |
| 3–4  | CRC-16/CCITT-FALSE of bytes 0–2 (polynomial 0x1021, initial value 0xFFFF) |

A receiver that does not know the version or the mode drops the frame.

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

## What the receiver reports (`rx.rs`)

For every frame: the position of its first sample in the stream, the frequency
offset and an estimate of the signal-to-noise ratio, taken from the tone
energies, with the noise counted over the whole band up to 8 kHz (add 4.3 dB
for the noise in a 3 kHz band). On a clean signal the estimate tops out near
26–39 dB depending on the frame length.

## Not there yet

Other modes, a resampler for 44.1/48 kHz devices, tracking of the symbol clock
inside a frame, the link layer (acknowledgements, sessions) and everything
above it: see [the plan](../plans/PLAN.md).
