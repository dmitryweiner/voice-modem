# Measurements

What the modem withstands, measured — first in simulation, later on real
channels. Each table says how to repeat it.

SNR is the ratio of the signal power to the power of white noise over the
whole band up to 8 kHz; in a 3 kHz band the same noise is 4.3 dB weaker.

## Mode R1 in simulation (2026-11-11)

`cargo run --release -p modem-sim --example sweep` — 40-byte frames (7 s),
50 per point.

| White noise, SNR | Frames received |
| ---------------- | --------------- |
| −15 dB and above | 50/50           |
| −16 dB           | 46/50           |
| −17 dB           | 33/50           |
| −18 dB           | 0/50            |

- **Frequency offset** (at −10 dB): received 50/50 at every offset tried from
  −250 to +250 Hz; the estimate is within 0.2 Hz on average.
- **Sound-card clock offset**, the longest frame (255 bytes, 34 s), no noise:
  received at every offset tried up to ±1000 ppm (±500 before the four sets
  of tones, see the next section). The receiver does not track the symbol
  clock; shorter frames tolerate proportionally more.
- **False frames**: none in 10 minutes of pure noise.
- No frame with a wrong payload was ever delivered: beyond the limit frames
  are lost or reported as damaged.

`crates/modem-sim/tests/link.rs` guards these with about 3 dB of margin, plus
a telephone band with echoes of 12–55 ms, a reverberant room, clipping of a
ten times overdriven signal and a signal 60 dB down.

## Mode R1 in a room, simulated (2026-11-11)

The same sweep. The room is a direct sound followed by a dense tail of
reflections that decays by 60 dB in the time RT60 (`Reverb` in `modem-sim`);
"direct" is the energy of the direct sound over that of all reflections — it
falls by 6 dB with every doubling of the distance. Noise is 20 dB below the
signal: it plays no part here. 40-byte frames, 25 rooms per point.

| RT60  | Direct | One set of tones (stage 3) | Four sets in turn |
| ----- | ------ | -------------------------- | ----------------- |
| 0.3 s | 0 dB   | 24/25                      | 25/25             |
| 0.3 s | −3 dB  | 12/25                      | 25/25             |
| 0.3 s | −6 dB  | 4/25                       | 25/25             |
| 0.3 s | −9 dB  | 1/25                       | 25/25             |
| 0.3 s | −12 dB | 3/25                       | 12/25             |
| 0.5 s | 0 dB   | 24/25                      | 25/25             |
| 0.5 s | −3 dB  | 11/25                      | 25/25             |
| 0.5 s | −6 dB  | 1/25                       | 25/25             |
| 0.5 s | −9 dB  | 0/25                       | 24/25             |
| 0.5 s | −12 dB | 1/25                       | 14/25             |

- With one set of tones the modem stopped where the reflections equal the
  direct sound, and the SNR it reported there was −6 to −8 dB — the numbers
  seen in the real room at 3 m just before it stopped (below). The "noise" it
  measured was its own echo.
- Four sets of tones used in turn move the limit by 9 dB, which in a room is
  nearly three times the distance. The real room agreed: 3 m became 8 m
  (below).
- Other things unchanged by the four sets: white noise (50/50 at −15 dB),
  frequency offsets, the codecs below. One thing improved by accident: the
  34 s frame now survives a clock offset of ±1000 ppm (it was ±500), because
  a window that has slipped onto the previous symbol finds no energy in its
  own tones instead of a wrong tone.
- Beyond −9 dB the preamble is the next thing to fail.

## The modes compared (2026-11-11)

`cargo run --release -p modem-sim --example modes` — 40-byte frames, 20 per
point, each with another frame number. The level a mode needs, for 19 or 20
frames of 20:

| Mode | Speed      | White noise, SNR | Room, RT60 0.3 s, direct | Room, RT60 0.5 s, direct |
| ---- | ---------- | ---------------- | ------------------------ | ------------------------ |
| R1   | 62.5 bit/s | −15 dB           | −9 dB or less            | −9 dB or less            |
| R2   | 125 bit/s  | −6 dB            | −9 dB or less            | −6 dB                    |
| R3   | 250 bit/s  | 3 dB             | −3 dB                    | −3 dB                    |
| R4   | 500 bit/s  | 9 dB             | 3 dB                     | 3 dB                     |
| F1   | 1400 bit/s | 3 dB             | 0 dB                     | 3 dB                     |
| F2   | 2800 bit/s | 9 dB             | 6 dB                     | 6 dB                     |
| F3   | 4200 bit/s | 12 dB            | 12 dB                    | 12 dB                    |

SNR is that of the header, a single tone at full level. The tones of the
faster robust modes split the same peak level between them, which is why
each doubling of their speed costs 6–9 dB. The carriers of the fast modes
are added up as they come and the rare peak is cut off, so each of the 64
keeps the power a single tone would have over the same noise — and
**F1 is no worse than R4 anywhere**: nearly three times the speed, 6 dB more
noise, the same room or a little more. The station therefore goes from R3
straight to F1 and leaves R4 for channels that carry tones but not phases.

What the receiver reports on such channels (the numbers the choice of mode
goes by): in white noise the SNR itself, on the header and on the carriers
alike; in the room the header reads 1–3 dB below the "direct" figure, and
the carriers from 1 dB below it (at 20 dB) to 3 dB above it (at 0 dB).

**The pause after the header.** In a room the last tones of the header, at
full level, ring into the first symbols of the payload, whose tones or
carriers are each 12–24 dB weaker. With the payload right after the header
R4 needed a direct sound of 9 dB, F1 3–6 dB, F2 12–15 dB and F3 15–18 dB,
and how a frame fared depended on its number — on which tones its header
ended with. 64 ms of silence in between (before every payload but one in
R1) gave the table above: 3–6 dB for every mode from R3 up.
`cargo run --release -p modem-sim --example pause` shows what still more
silence would give: another 64 ms, about 1 dB for F2 and F3.

Other things about the fast modes, same example and `tests/link.rs`:

- **Clock offset**, the longest frame (1023 bytes: 7.4 s in F1, 3.4 s in
  F3): F1 and F2 are received at every offset tried up to ±1000 ppm, F3 up
  to ±500 ppm.
- **Frequency offset**: received at ±200 Hz.
- A telephone band with echoes of 2 and 5 ms, and an input that cuts
  everything above 70 % of the level of the tones: received in all three.
- The receiver takes 14 ms for the carriers of the longest frame (native).

Through the codecs (`node scripts/codecs.mjs --mode r2` and so on; messages
of five that arrive):

| Codec                        | R1  | R2  | R3  | R4  | F1  | F2  | F3  |
| ---------------------------- | --- | --- | --- | --- | --- | --- | --- |
| G.711 µ-law, G.726 32 and 16 | 5   | 5   | 5   | 5   | 5   | 5   | 5   |
| G.723.1, 6.3 kbit/s          | 5   | 5   | 5   | 5   | 0   | 0   | 0   |
| AMR-NB, 12.2 kbit/s          | 5   | 5   | 5   | 5   | 5   | 4   | 1   |
| AMR-NB, 4.75 kbit/s          | 5   | 5   | 2   | 0   | 0   | 0   | 0   |
| Opus voice, 24 kbit/s        | 5   | 5   | 5   | 5   | 5   | 5   | 1   |
| Opus voice, 16 kbit/s        | 5   | 5   | 5   | 4   | 3   | 1   | 0   |
| Opus voice, 12 kbit/s        | 5   | 5   | 0   | 0   | 0   | 0   | 0   |
| Opus voice, 8 and 6 kbit/s   | 5   | 4   | 0   | 0   | 0   | 0   | 0   |
| iLBC, 15.2 kbit/s            | 4   | 4   | 4   | 4   | 0   | 0   | 0   |

The SNR the receiver measures on the header predicts most of this: where it
reads below 5 dB (AMR 4.75, Opus 12 and less) neither R3 nor F1 is advised.
It does not predict all: G.723.1 reads 6–8 dB and Opus at 16 kbit/s 11–13 dB,
and both lose the carriers. There a station tries F1, gets no answer, goes
on in R3 and tries again a minute later. Vocoders model a voice, and 64
steady carriers are not one; the waveform codecs (G.711, G.726) carry them
down to 16 kbit/s, and so does AMR at its highest rate.

## The choice of mode over the air (2026-11-11)

The build with the modes R1–R4 and the advice in acknowledgements, MacBook
and Android phone in a room, by the author:

| Distance   | SNR shown under received texts | Speed the station settled on |
| ---------- | ------------------------------ | ---------------------------- |
| next to it | +8 … +9 dB                     | 125–250 bit/s (R2, R3)       |
| about 8 m  | −3 … −1 dB                     | 62.5 bit/s (R1)              |

- The choice works on a real channel as in the simulated one: texts are
  delivered at both distances, and the speed follows the SNR (R3 from 5 dB;
  R4 would need 12 dB and was never advised).
- Next to each other the two devices read 8–9 dB, where a cable reads 26 dB
  and more. In the model of the room that is a direct sound 9–10 dB above
  the reflections — where F1 has 6–9 dB to spare and F2 about 3 dB. Whether small speakers and microphones pass 64 carriers at once
  as well as the model does is what the next test over the air will show.

Not measured: the fast modes over the air.

## Mode R1 through speech codecs (2026-11-10)

`node scripts/codecs.mjs` — five messages of 2 to 255 bytes (64 s) through
each codec with ffmpeg 8 and back, no other impairment. The SNR is what the
receiver estimated: here it measures the distortion by the codec.

| Codec                  | Messages | SNR, dB (min … max) |
| ---------------------- | -------- | ------------------- |
| none (16-bit WAV)      | 5/5      | 25.8 … 32.1         |
| G.711 µ-law, 64 kbit/s | 5/5      | 24.2 … 27.4         |
| G.726, 32 kbit/s       | 5/5      | 18.6 … 20.5         |
| G.726, 16 kbit/s       | 5/5      | 10.2 … 11.6         |
| iLBC, 15.2 kbit/s      | **4/5**  | −11.0 … −1.2        |
| G.723.1, 6.3 kbit/s    | 5/5      | 5.9 … 7.3           |
| AMR-NB, 12.2 kbit/s    | 5/5      | 11.6 … 12.8         |
| AMR-NB, 4.75 kbit/s    | 5/5      | 3.0 … 3.9           |
| Opus voice, 24 kbit/s  | 5/5      | 8.0 … 20.8          |
| Opus voice, 16 kbit/s  | 5/5      | 10.7 … 13.0         |
| Opus voice, 12 kbit/s  | 5/5      | 3.5 … 5.1           |
| Opus voice, 8 kbit/s   | 5/5      | 3.4 … 4.7           |
| Opus voice, 6 kbit/s   | 5/5      | 3.4 … 4.5           |

Measured with the frames of stage 3: one-way (broadcast) frames with the
7-byte header, messages cut into segments of 63 bytes — the 255-byte message
is five frames, all of which must arrive.

What this says about the main risk of the plan (vocoders leaving no usable
channel):

- 62.5 bit/s gets through every codec tried, down to 4.75 kbit/s, with one
  exception. **AMR-NB**, the codec of GSM and 3G calls, passes everything at
  both its highest and its lowest rate.
- **iLBC loses one of five messages** (two of five with the frames of stage
  1), and loses frames whole — the preamble or the header, not the payload.
  Not investigated yet; the first thing to look at when the
  telephone preset is tuned (stage 6).
- AMR-WB and EVS (the codecs of VoLTE calls) were not tested: ffmpeg has no
  encoder for them.
- The codec alone is not a call. No test here has discontinuous transmission
  (the voice detector that mutes "silence"), noise suppression, packet loss
  or time stretching by a jitter buffer — what phones and messengers add on
  top. The real answer comes from a real call.

## Speed of the receiver (2026-11-10)

M-series Mac. Searching for a preamble in noise:

| Build           | Device rate | Speed          |
| --------------- | ----------- | -------------- |
| native, release | 16 kHz      | 131× real time |
| WASM in Node 24 | 16 kHz      | 128× real time |
| WASM in Node 24 | 48 kHz      | 95× real time  |
| WASM in Node 24 | 44.1 kHz    | 97× real time  |

At 48 kHz the sample rate converter is a quarter of the work. With a sine
computed per filter tap it was four fifths (22× real time); it now reads the
filter from a table.

The worst case is not noise but **a frame whose beginning was missed**: its
tones cross the preamble threshold about 60 times a second, and each crossing
starts a search for the exact peak and the second chirp. Native: 18× real
time, 36× since that search looks at every fourth position first. Not yet
measured on a phone, where it matters.

## Over the air between real devices (2026-11-10)

The stage 2 build (one-way text, no repeats), by the author, in a room:

| From → to                                   | Result               |
| ------------------------------------------- | -------------------- |
| MacBook Pro → the same MacBook, another tab | received, SNR +10 dB |
| Android phone (Chrome) → MacBook Pro        | received, SNR +12 dB |
| MacBook Pro → Android phone (Chrome)        | received, SNR +7 dB  |

- **Range: about 4 m.** Beyond it nothing is received, and "heard but could
  not be read" never appeared — at that time it was shown only for a bad
  payload, so the loss is in the preamble or the header.
- The SNR at arm's length is 15–20 dB below what the same signal gives
  through a cable (above 26 dB), and 22–27 dB above where white noise stops
  the modem. So it is hardly noise that limits the range. The suspects are
  what a room and small speakers do and white noise does not: echoes longer
  than a symbol, and distortion of the chirps. To be found out before any
  tuning; unreadable headers after a clear preamble are now reported, which
  tells the two apart.
- iPhone and a phone call in between: not tried yet.

The stage 3 build (acknowledgements and repeats), same room, MacBook and
Android phone: texts are delivered and acknowledged both ways, greetings and
statuses work. Received texts show SNR −2, −4, −6 dB as the distance grows;
**at about 3 m** frames are "heard but could not be read" and nothing gets
through. The simulated room above gives the same picture, which is what
pointed at reverberation; the four sets of tones are the answer to it.

The build with four sets of tones (2026-11-11), same room and devices:
**delivery is reliable at about 8 m** — where the model said three times the
3 m would be. The far limit was not looked for.

On a Mac the display must be awake for a browser to process sound at all:
with the display asleep an AudioContext reports "running" while its clock
stands still. `npm run smoke` keeps it awake with `caffeinate`.
