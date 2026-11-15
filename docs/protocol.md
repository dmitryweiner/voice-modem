# Protocol

The sound on the wire and the rules of the exchange, as implemented in
`crates/modem-core`. Version 0: it changes freely until the first release.
Every number here has a test in the module named next to it.

All processing runs at **16 kHz**; the core converts from and to the sample
rate of the audio device (`resample.rs`). Bits are sent most significant first;
multi-byte numbers are big-endian.

## Frame

```
[lead-in tone] [preamble: up-chirp, down-chirp] [header] [block + CRC-32] [block + CRC-32] …
   optional              256 ms                  992 ms    mode R1: 128 ms per byte + 608 ms per block
```

Between the header and a payload in any mode but R1 there are 64 ms of
silence: in a room the last tones of the header ring on, far louder than
each of the many tones or carriers of such a payload (`rx.rs`,
`PAYLOAD_PAUSE`).

A data or broadcast frame carries 0–4095 bytes of payload in blocks; an
acknowledgement ends after the header. The frame fades in and out over 32
samples, except that a payload of carriers ends as its last symbol does
(`tx.rs`).

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
| 3    | bits 7–4: frame number (0–15); bits 3–0: the four highest bits of the payload length       |
| 4    | the eight lowest bits of the payload length in bytes                                       |
| 5–6  | CRC-16/CCITT-FALSE of bytes 0–4 (polynomial 0x1021, initial value 0xFFFF)                  |

| Kind | Name            | Address and number                                    | Payload                                                           |
| ---- | --------------- | ----------------------------------------------------- | ----------------------------------------------------------------- |
| 0    | data            | of the sender; it counts its frames, 0 again after 15 | yes; asks for an acknowledgement                                  |
| 1    | acknowledgement | those of the data frame it answers                    | none: the frame ends with the header; the length field: see below |
| 2    | broadcast       | of the sender                                         | yes; expects no answer                                            |

A receiver that does not know the version, the kind or the mode drops the
frame.

In an acknowledgement the length field holds, in bits 3–0, the advised mode
([below](#choosing-the-mode)) and in bits 11–4 the number of blocks at the end
of the acknowledged payload that did not arrive whole or came after one that
did not; 0 when all of it arrived ([below](#acknowledgement-and-repeats)).

### Payload

The payload goes in blocks, in the mode named by the header: all but the last
as long as the mode allows (64 bytes in R1, 128 in R2, 255 in R3 and R4, 1023
in F1–F3), the last with the rest; an empty payload is one empty block. Each
block is followed by its CRC-32 (IEEE 802.3, the one of zip and PNG) and is
coded on its own, so that a block damaged on the way costs only itself and
what comes after it.

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
and the grid is shared by `S` sets that take turns: in symbol number `n` of
the header or of the payload (each counted from 0, on through all the blocks
of a payload), tone `t` of group `g` has the frequency

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

The header, and each block of the payload (with its CRC) of a robust mode,
are coded as blocks on their own, one after another:

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

The blocks of the payload (each with its CRC-32) are coded one after another,
then carried by OFDM symbols after two training symbols:

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
robust modes (started once per block); the blocks one after another, then
more of the sequence of the last one up to a whole number of symbols.

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
  each take the next 2 or 4 bits on the air.
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

| Type     | Content of the message                                                                                                                           |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0 text   | UTF-8 text, cut into segments anywhere, also inside a character; at most 1000 bytes                                                              |
| 1 hello  | capabilities, 2 bytes (bit 0: the station seals what it sends), then the name of the station, up to 32 bytes of UTF-8; one segment; never sealed |
| 2 file   | a message of the file transfer ([below](#files-filesrs)), at most 4091 bytes                                                                     |
| 3 text   | a text packed ([below](#packed-text-textrs)): sent when that is shorter than its UTF-8                                                           |
| 4 sealed | a message of type 0, 2 or 3 sealed by a password ([below](#sealed-messages-sealrs))                                                              |

A receiver starts a message at a segment with the "first" flag and delivers
it at the one without "more". A segment adds the bytes from its offset on
that are not there yet; one that starts beyond what was received drops the
message, and segments that come without a first one are dropped.

### Packed text (`text.rs`)

Chat in English and Russian, character by character in a fixed Huffman code:
a Russian text takes about 40% of its UTF-8, an English one 60–70%
([measurements](measurements.md)). There are two codes, one for after a
Latin letter (a–z, A–Z) and one for after a Cyrillic one (а–я, ё, А–Я, Ё);
any other character keeps the code of the letter before it.

- Symbols: 0 is the end of the text, 1 the escape, then the characters of
  the table `CHARACTERS` in `text.rs`, in its order: space, line feed, the
  Latin and the Cyrillic letters, digits and punctuation. A character not
  in the table is the escape and 21 bits of its code point.
- Each code comes from Huffman's algorithm over the weights of its column
  in that table (and 2000 for the end, 300 for the escape): the two lightest
  trees are joined, among equal weights the one made first; leaves are made
  in the order of the symbols, before any tree. Codes are canonical, as in
  deflate: shorter codes first, those of one length consecutive in the order
  of the symbols. The weights are part of the protocol.
- On the air, most significant bit first: one bit for the code of the start
  (1: that of the Cyrillic letters; the code of the first letter of the text,
  Latin when there is none), the code of each character, the end, zero bits
  up to a whole byte.

A packed text that does not unpack is acknowledged and dropped.

### Sealed messages (`seal.rs`)

A station given a password seals every text and every message of the file
transfer it sends; the greeting goes in the clear. A message is sealed
whole, before it is cut into segments, so a long one pays for the seal once.

- **Key**: Argon2id (version 0x13) of the password, the salt the 23 bytes
  `voice-modem/password/v0`, 64 MiB, 3 passes, 1 lane, 32 bytes. The salt
  is the same everywhere: both stations come to the key from the password
  alone, with no exchange before. A guess costs a tenth of a second here
  and the same for everyone, so a password is as good as it is long.
- **Seal**: ChaCha20-Poly1305 (RFC 8439). The plaintext is the type of the
  message (one byte) and its content; the associated data is the address of
  the sending station, 2 bytes. On the air: the nonce (12 bytes), the
  ciphertext, the tag (16 bytes) — 29 bytes more than the message, the type
  included.
- **Nonce**: 8 random bytes the application gives at the start, then a
  counter of the messages sealed, 4 bytes; after 2³² messages the 8 bytes
  become splitmix64 of themselves.
- A station opens a sealed message with its own password. One that does
  not open — no password, another one, or changed on the way — is
  acknowledged (it arrived) and reported as unopened. A station with a
  password reads unsealed messages too, and says which came sealed.

What stays in the clear: frame headers (who sends, how long, when — anyone
hears that the channel is busy), the greeting with the name of the
station, acknowledgements. A recorded sealed message played again is
accepted again, as an unsealed one would be. Encryption is forbidden on
amateur radio bands; the app says so next to the password.

### Acknowledgement and repeats

- A station that receives a **data** frame answers with an acknowledgement
  0.5 s after the frame ended, whether or not it has seen the frame before.
  It delivers a payload only if the frame number differs from the last one
  received from that address: a repeat means the acknowledgement was lost.
- The receiver takes the blocks of a payload that arrived whole, up to the
  first that did not, and says in the acknowledgement how many blocks at the
  end it misses. The sender counts the blocks before those as received: the
  rest of the segment goes as the next one. A message ends only with a
  segment that arrived whole. A frame whose first block is damaged is not
  acknowledged at all. A repeat is answered with what was missing the first
  time, whatever arrived this time.
- The sender waits for the acknowledgement until 0.5 s + the length of an
  acknowledgement + 1 s after its frame ended (10 s more with push-to-talk).
  Then it waits a random 0.2–1.2 s and sends the frame again under the same
  number — the same segment, or its beginning if the mode is slower now and
  takes less (the rest follows as further segments). Before each further
  repeat it waits from 0.2 s up to twice as long as the time before: 2.2 s,
  4.2 s, 8.2 s. A station hears another one's frame only some 0.3 s after
  it started, so two stations that collided collide again whenever their
  waits are that close; in a second, half of them are. After five
  transmissions of a frame the message has failed and what is left of it is
  dropped.
- The next segment goes out only after the previous one was acknowledged
  (stop and wait).
- A **broadcast** frame is sent once; its segments follow each other 0.2 s
  apart.

### Choosing the mode

- A frame carries as many blocks as last twelve seconds: one in R1, R2 and
  R3 (64, 128 and 255 bytes, 8–9 s), two in R4 and F1 (510 and 2046 bytes),
  four in F2 and F3 (4092 bytes, 12 s and 8 s). Every frame costs more than
  a second of preamble and header and about 2.4 s for its acknowledgement
  and the turn; a longer frame spends less of its time on those, but keeps a
  text waiting longer. A message is cut as it is sent, each segment to the
  size of the mode at that moment; a fast frame takes a whole text.
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
  if that is slower, and one mode faster if the advice is faster and the
  frame arrived whole. When a frame is not acknowledged it is repeated one
  mode slower, and the mode that failed is not used again for a minute,
  whatever the advice. After two minutes without an acknowledgement it
  starts from R1 again.
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

## Files (`files.rs`)

A file goes as messages of type 2 over the link layer, each acknowledged
like a text (over a one-way channel: [below](#over-a-one-way-channel-fountainrs)). The first byte of such a message is its kind, the second the
number of the transfer, which the sender counts from 0 (0 again after 255):

| Kind       | From             | After the two bytes                                                                                                                                       |
| ---------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0 offer    | sender           | length of the file, 4 bytes; CRC-32 of its content, 4 bytes; encoding, 1 byte; length on the air, 4 bytes; its name, up to 255 bytes UTF-8                |
| 1 accept   | receiver         | the offset on the air the content is wanted from, 4 bytes                                                                                                 |
| 2 data     | sender           | the offset of the chunk on the air, 4 bytes; the chunk                                                                                                    |
| 3 abort    | sender           | nothing: the sender gives the transfer up                                                                                                                 |
| 4 refuse   | receiver         | nothing: the receiver declines the offer or gives the transfer up                                                                                         |
| 5 announce | sender (one way) | length of the file, 4 bytes; CRC-32, 4 bytes; encoding, 1 byte; length on the air, 4 bytes; bytes of a symbol, 2 bytes; its name, as much as fits a frame |
| 6 symbol   | sender (one way) | the number of the symbol, 2 bytes; the symbol                                                                                                             |

- **On the air** the content goes deflated (RFC 1951, level 9) when that is
  shorter (encoding 1), or as it is (encoding 0). Offsets and lengths of
  chunks count the bytes on the air; the progress shown counts bytes of the
  file in proportion. A receiver declines an offer of an encoding it does
  not know.
- The sender offers; nothing of the content goes before the **accept**,
  which is the decision of the human at the receiver. The sender waits for
  it as long as it takes.
- Chunks go in order, one at a time, each acknowledged before the next
  (stop and wait, as everything on the link). A chunk is as long as one
  frame of the mode of the moment takes: 57 bytes of the file in R1, 2039
  in F1, 4085 in F2 and F3, and 29 bytes less with a password (the seal of
  each message). If that frame is repeated in a slower mode, or
  arrives in part, the link layer sends the rest as further segments, as it
  does with a text.
- A chunk is handed to the link layer only when nothing else waits there:
  **a text goes ahead of the rest of a file**. A station sends one file at a
  time; further ones wait, each offered when the one before is through.
- The receiver appends the bytes of a chunk it does not have yet. To a chunk
  from beyond what it has it answers with an accept that names the offset it
  needs, and the sender goes on from there. To a chunk of a transfer it does
  not know it answers with a refuse.
- When it has as many bytes on the air as the offer said, the receiver
  inflates them if they are deflated (never past the length of the file),
  checks the length and the CRC-32 and delivers the file, or drops it if
  either is wrong. The sender takes the file for delivered when its last
  chunk is acknowledged.
- **Going on after a break.** When a message of the transfer is not
  acknowledged after every attempt, the transfer has failed at the sender;
  the receiver keeps what it has. An offer with the same name, length,
  checksum, encoding and length on the air as a file the receiver has in
  part — from any station, under any number — continues that file: if it
  was accepted before, the accept goes out at once, with the offset of the
  first byte that is missing.
- A receiver keeps at most 8 files offered or unfinished; the oldest gives
  way.

Frames have no destination, so a file, as a text, is for the one other
station on the channel.

### Over a one-way channel (`fountain.rs`)

A station that expects no answers (broadcast frames) cannot ask for consent
nor learn what arrived. It broadcasts a file instead, for whoever listens:

- **Pieces.** The bytes on the air (deflated when that is shorter, as
  above) are cut into `k` pieces of `T` bytes, as few as frames take
  (`k = ⌈L / (room − 4)⌉`, where `room` is what one frame of the mode takes
  of a message) and as even as can be (`T = ⌈L / k⌉`); the last is padded
  with zeros. At most 256 pieces: up to 15 KB in R1, 64 KB in R3, 1 MB in
  F2 and F3.
- **Symbols.** Symbol `n` for `n < k` is piece `n`. Every further symbol is
  the sum over all pieces of piece `j` times the coefficient `c_j`, in
  GF(256) with the polynomial x⁸ + x⁴ + x³ + x² + 1 (0x11D); the
  coefficients are the bytes of splitmix64 seeded with `n`, the least
  significant byte of each output first: `state += 0x9E3779B97F4A7C15`;
  `z = state`; `z = (z ^ z >> 30) · 0xBF58476D1CE4E5B9`;
  `z = (z ^ z >> 27) · 0x94D049BB133111EB`; `z ^= z >> 31`.
- **What goes.** An announce, then the symbols from 0 on, one per frame,
  with the announce again before every eighth: a station that tunes in late
  learns what the file is. It goes until the sender stops it, or after
  `10 · k` symbols (65 536 at most).
- **What a listener does.** It keeps symbols heard before the announce (256
  at most), then solves for the pieces as symbols come (Gaussian
  elimination over GF(256)); any `k` independent symbols give the file:
  nearly always the first `k` that arrive, one or two more now and then.
  Then it takes the first `L` bytes, inflates them if need be and checks
  the length and the CRC-32. What comes of that broadcast after is
  ignored. A listener keeps at most 8 broadcasts at once; the oldest gives
  way. A station that takes part in a two-way exchange collects broadcasts
  too.

## What the receiver reports (`rx.rs`)

For every frame: the position of its first sample in the stream, the frequency
offset and an estimate of the signal-to-noise ratio, taken from the tone
energies, with the noise counted over the whole band up to 8 kHz (add 4.3 dB
for the noise in a 3 kHz band). On a clean signal the estimate tops out near
23–33 dB depending on the frame length. For a fast frame also the
signal-to-noise ratio on its carriers: the power of a carrier over what the
pilots were off by, up to the end of the last block that arrived whole — a
burst of noise that took a later block says nothing about the channel. In
white noise the two agree.

For a frame whose payload arrived in part: the blocks before the first
damaged one and how many blocks are missing. A frame whose first block is
damaged, and a preamble that is unmistakable (both chirps make up a quarter
of the energy of their windows or more) but is followed by an unreadable
header, are reported as "heard but not read".

## Not there yet

More bits per carrier and bit loading by carrier, keeping blocks that
arrive after a damaged one, tracking of the symbol clock inside a frame of
tones, addressed sessions, protection against a recording played again,
presets for the kinds of channel: see [the plan](../plans/PLAN.md).
