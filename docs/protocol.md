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

Between the header and a payload in any mode but R1 there are 64 ms of
silence: in a room the last tones of the header ring on, far louder than
each of the many tones or carriers of such a payload (`rx.rs`,
`PAYLOAD_PAUSE`).

A data or broadcast frame carries 0–1023 bytes of payload; an acknowledgement
ends after the header. The frame fades in and out over 32 samples, except
that a payload of carriers ends as its last symbol does (`tx.rs`).

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
| 3    | bits 7–2: frame number (0–63); bits 1–0: the two highest bits of the payload length        |
| 4    | the eight lowest bits of the payload length in bytes                                       |
| 5–6  | CRC-16/CCITT-FALSE of bytes 0–4 (polynomial 0x1021, initial value 0xFFFF)                  |

| Kind | Name            | Address and number                                    | Payload                                                                      |
| ---- | --------------- | ----------------------------------------------------- | ---------------------------------------------------------------------------- |
| 0    | data            | of the sender; it counts its frames, 0 again after 63 | yes; asks for an acknowledgement                                             |
| 1    | acknowledgement | those of the data frame it answers                    | none: the frame ends with the header; the length is the advised mode (below) |
| 2    | broadcast       | of the sender                                         | yes; expects no answer                                                       |

A receiver that does not know the version, the kind or the mode drops the
frame.

### Payload

The payload bytes followed by their CRC-32 (IEEE 802.3, the one of zip and
PNG), in the mode named by the header.

## Modes

| Id  | Name | Signal                                      | Code               | Net speed  |
| --- | ---- | ------------------------------------------- | ------------------ | ---------- |
| 0   | R1   | 1 tone, one of 16, four sets in turn        | convolutional, 1/2 | 62.5 bit/s |
| 1   | R2   | 4 tones at once, each one of 4, four sets   | convolutional, 1/2 | 125 bit/s  |
| 2   | R3   | 8 tones at once, each one of 4, two sets    | convolutional, 1/2 | 250 bit/s  |
| 3   | R4   | 16 tones at once, each one of 4, one set    | convolutional, 1/2 | 500 bit/s  |
| 4   | F1   | 56 carriers of 2 bits (QPSK) and 8 pilots   | LDPC, 1/2          | 1400 bit/s |
| 5   | F2   | 56 carriers of 4 bits (16-QAM) and 8 pilots | LDPC, 1/2          | 2800 bit/s |
| 6   | F3   | 56 carriers of 4 bits (16-QAM) and 8 pilots | LDPC, 3/4          | 4200 bit/s |

All modes share the grid of 64 frequencies 31.25 Hz apart from 750 Hz to
2718.75 Hz. The header of every frame is in R1.

The robust modes (R) are tones compared by energy: the receiver needs neither
the phase nor the level, which is what survives vocoders and long echoes. The
fast modes (F) are coherent: every carrier sounds all the time and carries
bits in its phase and amplitude. They need a channel that keeps the phase and
has little echo beyond 8 ms — a cable, a line, a speaker close by.

## Robust modes

Symbols of 512 samples (32 ms), no gaps between them.

### Modulation (`mfsk.rs`)

A symbol is `G` tones sounding at once, one from each of `G` groups; a tone is
one of `M` and carries log₂ M bits. The groups lie side by side on the grid,
and the grid is shared by `S` sets that take turns: in symbol number `n` of a
block (the header and the payload are blocks of their own, each counted from
0), tone `t` of group `g` has the frequency

    750 + 31.25 · ((g · M + t) · S + n mod S) Hz.

The first log₂ M bits of the symbol choose the tone of group 0, and so on.
Tone `t` carries the value `t XOR (t >> 1)` (Gray code: neighbouring tones
differ in one bit). The phase of each group is continuous from symbol to
symbol; each tone has 1/G of the amplitude, so the sum never exceeds it.

Why the sets: in a room a tone rings on for a few tenths of a second — ten
symbols. With one set of tones that echo lies exactly where the receiver
looks for the next symbols, and a few metres from the speaker it is louder
than the direct sound. With four sets the echo of the last three symbols
falls between the tones in use. The faster modes trade this away, and the
power of each tone, for bits ([measurements](measurements.md)).

The receiver measures the energy of each tone of the symbol's set and never
needs the phase or the level. The soft value of a bit is the amplitude of the
loudest tone of its group that has the bit clear minus that of the loudest
that has it set.

### Block coding (`coding.rs`, `conv.rs`)

The header, and the payload (with its CRC) of a robust mode, are each coded
as a block on their own:

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

## Fast modes

The payload block (the payload and its CRC-32) is coded, then carried by
OFDM symbols after two training symbols:

```
[training] [training] [data symbol 0] [data symbol 1] …
   40 ms      40 ms        40 ms
```

### Code (`ldpc.rs`)

The quasi-cyclic LDPC codes of IEEE 802.11n with code words of 648 bits:
324 information bits at rate 1/2, 486 at rate 3/4. The parity-check matrix
is the table of the standard for that length and rate, 27 × 27 blocks: row
`r` of a block with the entry `s` has its one in column `(r + s) mod 27`. A
code word is the information bits followed by the parity bits.

A block of `B` bytes takes `W = ⌈8B / K⌉` code words (`K` information bits
each; never less than one). Its bits go into the words in order, `⌊8B / W⌋`
to each and one more to the first `8B mod W`. The information bits a word has
no use for are zeros and are not sent (shortening); every word is sent with
all its parity bits. On the air: word after word, the bits of the block and
then the parity bits of each, all XORed with the scrambler sequence of the
robust modes (started once per block), then more of that sequence up to a
whole number of symbols.

The receiver decodes each word from soft values (layered min-sum).

### Symbols (`ofdm.rs`)

A symbol is 640 samples: 512 samples in which carrier `k` (0–63, at
750 + 31.25 · k Hz) has the amplitude and phase of its complex value `X`,
the phase counted from the first of them, preceded by a copy of the last 128
(the cyclic prefix, 8 ms). All carriers have the same mean power, set so
that the peak allowed is 2.8 times the root mean square of the sum; what
exceeds it is cut off.

- **Training symbol**: every carrier has the value exp(iπk² / 64).
- **Data symbol** number `n` (from 0): the carriers with `(k − 3n) mod 8 = 0`
  are pilots with their training value; the other 56, in the order of `k`,
  each take the next 2 or 4 bits of the block.
- **Bits to value**: the first half of the bits of a carrier gives the real
  part, the second half the imaginary part. One bit: 0 → −1, 1 → +1. Two
  bits: 00 → −3, 01 → −1, 11 → +1, 10 → +3. The value is divided by √2 or
  √10 to a mean power of 1.

The receiver takes the picture of the channel from the training symbols and
follows the phase with the pilots, which also refresh that picture: in eight
symbols every carrier has been a pilot. Its window starts in the middle of
the prefix, so the clocks of the two sound cards may drift apart by 4 ms
during a frame.

## Link layer (`link.rs`)

Stations take turns on one channel. A station picks its 16-bit address at
random when it starts.

### Payload of data and broadcast frames

The first byte is the type in bits 3–0 and flags above: bit 6 — the first
segment of a message, bit 7 — more segments follow. In every segment but the
first, two bytes follow with the offset of the segment in the message. An
empty payload and unknown types are acknowledged and otherwise ignored.

| Type    | Content of the message                                                                                          |
| ------- | --------------------------------------------------------------------------------------------------------------- |
| 0 text  | UTF-8 text, cut into segments anywhere, also inside a character; at most 1000 bytes                             |
| 1 hello | capabilities, 2 bytes (none defined yet: 0), then the name of the station, up to 32 bytes of UTF-8; one segment |

A receiver starts a message at a segment with the "first" flag and delivers
it at the one without "more". A segment adds the bytes from its offset on
that are not there yet; one that starts beyond what was received drops the
message, and segments that come without a first one are dropped.

### Acknowledgement and repeats

- A station that receives a **data** frame answers with an acknowledgement
  0.5 s after the frame ended, whether or not it has seen the frame before.
  It delivers a payload only if the frame number differs from the last one
  received from that address: a repeat means the acknowledgement was lost.
- The sender waits for the acknowledgement until 0.5 s + the length of an
  acknowledgement + 1 s after its frame ended (10 s more with push-to-talk).
  Then it waits a random 0.2–1.2 s and sends the frame again under the same
  number — the same segment, or its beginning if the mode is slower now and
  takes less (the rest follows as further segments). After five
  transmissions of a frame the message has failed and what is left of it is
  dropped.
- The next segment goes out only after the previous one was acknowledged
  (stop and wait).
- A **broadcast** frame is sent once; its segments follow each other 0.2 s
  apart.

### Choosing the mode

- A frame carries up to 64 bytes in R1, 128 in R2, 255 in R3 and R4 and
  1023 in the fast modes: a frame has more than a second of preamble and
  header, and a faster mode should not spend its time on those. A message is
  cut as it is sent, each segment to the size of the mode at that moment; a
  fast frame takes a whole text.
- The modes a station chooses from are R1, R2, R3, F1, F2, F3, in this
  order. R4 is not among them: F1 is faster and withstands more noise and
  no less echo. R4 is for channels that carry tones but not phases, and is set
  by hand.
- An acknowledgement carries **advice** in the length field: the id of the
  fastest mode its sender expects to receive, from what it measured on the
  acknowledged frame (echo counts as noise there). From the signal-to-noise
  ratio of the header tones: R2 from −1 dB, R3 from 5 dB, F1 from 6 dB. From
  that on the carriers, when the frame was a fast one: F1 from 6 dB, F2 from
  11 dB, F3 from 16 dB, and no more than R3 below that. Each is about 3 dB
  above where the mode stops in white noise and in a simulated room.
- A station starts in R1. On an acknowledgement it moves to the advised mode
  if that is slower, and one mode faster if the advice is faster. When a frame
  is not acknowledged it is repeated one mode slower, and the mode that failed
  is not used again for a minute, whatever the advice. After two minutes
  without an acknowledgement it starts from R1 again.
- Each direction has its own mode. A station can instead be fixed to one
  mode; broadcast frames always are.

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
23–33 dB depending on the frame length. For a fast frame also the
signal-to-noise ratio on its carriers: the power of a carrier over what the
pilots were off by. In white noise the two agree.

A frame whose payload checksum fails, and a preamble that is unmistakable
(both chirps make up a quarter of the energy of their windows or more) but is
followed by an unreadable header, are reported as "heard but not read".

## Not there yet

More bits per carrier and bit loading by carrier, several frames per
acknowledgement, tracking of the symbol clock inside a frame of tones,
addressed sessions, files and everything else above the link layer: see [the plan](../plans/PLAN.md).
